# frozen_string_literal: true

require_relative "production_text"
require_relative "video_qa"
require_relative "media_ref"
require_relative "production_media_root"

class ProductionVisualAudit
  include ProductionMediaRoot
  RUBRIC = "production-visual-v1"
  SHEETS_PER_CALL = 4
  REFERENCES_PER_CALL = 8
  CATEGORIES = %w[politics ethnicity_religion crime sexual violence minors superstition values infringement advertising
                  injury blood text_overlay identity scene_jump continuity framing hook pacing arc character dialogue shootability cost other].freeze

  def initialize(text:, video_qa:, media_root:)
    @text, @video_qa, @root = text, video_qa, media_root
  end

  def review_source(drama:, path:, context:, identity:, update:, character_ids: nil)
    require!(File.file?(path.to_s), "production_visual_source_missing")
    sha = Digest::SHA256.file(path).hexdigest
    references = reference_images(drama, character_ids)
    built = @video_qa.material({ "id" => "production-#{identity}", "outputPath" => path })
    require!(built["ok"] && !built["imagePaths"].empty?, "production_visual_preparation_failed")
    milliseconds = built.dig("autoChecks", "durationMs").to_i
    plans = plan(built["imagePaths"], references, milliseconds)
    key = ScreenplayRules.digest([RUBRIC, identity, sha, context, references.map { |entry| entry.slice("id", "sha256") }])
    record = drama["production"].fetch("visualAudits", {})[key] || { "identity" => identity, "sourceSHA256" => sha, "parts" => [] }
    return record["result"] if record["result"]
    index = record["parts"].length
    require!(index < plans.length, "production_visual_coverage_invalid")
    selected = plans[index]
    instruction = '实际查看本次全部附件，只审画面，声音由独立音频审核负责。参考形象图在前，带时间网格的画面拼图在后。每个参考人物都要核对身份外观与服装；按网格遍历完整时间范围。检查原剧本及分镜动作是否落实、身份连续、场景与道具、口型动作、肢体、叠字、类品牌、伤痕及内容合规；剧本要求的剪辑切镜不能误判成随机跳切。输出 {status:pass|warn|block,summary,coverage:{firstMs,lastMs},referenceIDs:[...],issues:[{rule,level:must|advice,reason,suggestion,firstMs,lastMs}]}。coverage与referenceIDs必须逐字等于expectedCoverage、expectedReferenceIDs，问题时间用整个视频的绝对毫秒。未知或未读取全部附件必须block，不能据文字描述猜测通过。rule使用允许的categories之一。'
    paths = selected["references"].map { |entry| entry["filePath"] } + selected["images"]
    fingerprints = paths.map { |file| Digest::SHA256.file(file).hexdigest }
    answer = @text.checked("独立画面审核", instruction, { "context" => context, "sourceSHA256" => sha,
      "expectedCoverage" => selected["coverage"], "expectedReferenceIDs" => selected["references"].map { |entry| entry["id"] },
      "references" => selected["references"].map { |entry| entry.slice("id", "name") }, "categories" => CATEGORIES,
      "firstSheetIndex" => selected["sheetIndex"], "sheetLayout" => built["autoChecks"]["sheetLayout"],
      "durationMs" => milliseconds, "autoChecks" => built["autoChecks"] }, image_paths: paths) do |value|
      validate!(value, selected, milliseconds)
    end
    require!(Digest::SHA256.file(path).hexdigest == sha && paths.map { |file| Digest::SHA256.file(file).hexdigest } == fingerprints, "production_visual_source_changed")
    record["parts"] << answer.merge("attachedImages" => paths.length, "imageSHA256" => fingerprints, "sheetIndex" => selected["sheetIndex"])
    record["result"] = finish(record, plans, milliseconds, sha) if record["parts"].length == plans.length
    update.call { |current| (current["production"]["visualAudits"] ||= {})[key] = record }
    record["result"] || { "pending" => true, "viewedParts" => record["parts"].length, "totalParts" => plans.length }
  end

  private

  def require!(condition, code)
    ProductionTextRules.require!(condition, code)
  end

  def reference_images(drama, ids)
    characters = ids ? drama["characters"].select { |entry| ids.include?(entry["id"]) } : drama["characters"]
    characters.map do |character|
      media = MediaRef.selected(character, production_media_root)
      require!(media && File.file?(media["filePath"].to_s), "production_visual_identity_missing")
      { "id" => character["id"], "name" => character["name"], "filePath" => media["filePath"], "sha256" => Digest::SHA256.file(media["filePath"]).hexdigest }
    end
  end

  def plan(images, references, milliseconds)
    groups = references.empty? ? [[]] : references.each_slice(REFERENCES_PER_CALL).to_a
    sheet_ms = (VideoQA::SHEET_ROWS * VideoQA::SHEET_COLUMNS * 1000.0 / VideoQA::SHEET_FPS).round
    images.each_slice(SHEETS_PER_CALL).each_with_index.flat_map do |files, index|
      first = index * SHEETS_PER_CALL
      coverage = { "firstMs" => first * sheet_ms, "lastMs" => [milliseconds, (first + files.length) * sheet_ms].min }
      groups.map { |group| { "images" => files, "references" => group, "sheetIndex" => first + 1, "coverage" => coverage } }
    end
  end

  def validate!(value, selected, milliseconds)
    require!(value.is_a?(Hash) && %w[pass warn block].include?(value["status"]), "production_visual_verdict_invalid")
    ProductionTextRules.text!(value["summary"], "visual summary", 1000)
    require!(value["coverage"] == selected["coverage"] && value["referenceIDs"] == selected["references"].map { |entry| entry["id"] }, "production_visual_coverage_invalid")
    require!(value["issues"].is_a?(Array) && value["issues"].length <= 20, "production_visual_issues_invalid")
    value["issues"].each do |issue|
      require!(issue.is_a?(Hash) && CATEGORIES.include?(issue["rule"]) && %w[must advice].include?(issue["level"]), "production_visual_issue_invalid")
      %w[reason suggestion].each { |key| ProductionTextRules.text!(issue[key], key, 1000) }
      require!(issue["firstMs"].is_a?(Integer) && issue["lastMs"].is_a?(Integer) && issue["firstMs"].between?(selected["coverage"]["firstMs"], selected["coverage"]["lastMs"]) &&
        issue["lastMs"].between?(issue["firstMs"], [milliseconds, selected["coverage"]["lastMs"]].min), "production_visual_issue_time_invalid")
    end
    require!(value["status"] != "pass" || value["issues"].none? { |issue| issue["level"] == "must" }, "production_visual_status_conflict")
  end

  def finish(record, plans, milliseconds, sha)
    cursor = 0
    spans = plans.map { |entry| entry["coverage"] }.uniq
    spans.each { |span| require!(span["firstMs"] == cursor && span["lastMs"] > cursor, "production_visual_time_gap"); cursor = span["lastMs"] }
    require!(cursor == milliseconds && record["parts"].length == plans.length, "production_visual_coverage_incomplete")
    issues = record["parts"].flat_map do |part|
      found = part["issues"].dup
      if part["status"] == "block" && found.none? { |issue| issue["level"] == "must" }
        found << { "rule" => "other", "level" => "must", "reason" => part["summary"], "suggestion" => "核对画面后重新生成并复审。", "firstMs" => part["coverage"]["firstMs"], "lastMs" => part["coverage"]["lastMs"] }
      end
      found
    end
    { "accepted" => issues.none? { |issue| issue["level"] == "must" }, "viewedFrames" => true, "sourceSHA256" => sha,
      "durationMs" => milliseconds, "issues" => issues, "coverage" => spans, "referenceCoverage" => plans.map { |entry| entry["references"].map { |ref| ref["id"] } }.uniq,
      "attachedImages" => record["parts"].sum { |part| part["attachedImages"] } }
  end
end

# frozen_string_literal: true

require_relative "production_film_audit"
require_relative "production_media_root"

class ProductionSeriesReview
  include ProductionMediaRoot
  BOUNDARY_MS = 3000
  def initialize(film:, visual:, audio:, text:, media_root:)
    @film, @visual, @audio, @text, @root = film, visual, audio, text, media_root
  end

  def step(drama:, arguments:, meter:, update:)
    input = arguments["input"]
    coverage = input["range"] ? (input["range"]["first"]..input["range"]["last"]).to_a : (1..drama["episodeCount"]).to_a
    accepted = coverage.map do |order|
      review = @film.accepted(drama, order)
      require!(review, "production_series_film_not_accepted")
      review["artifact"]
    end
    basis = ScreenplayRules.digest(accepted.map { |artifact| artifact.slice("episodeOrder", "sha256", "sourceDigest") })
    key = input["range"] ? "#{coverage.first}-#{coverage.last}" : "all"
    prior = drama["production"].fetch("seriesReviews", {})[key]
    pairs = input["range"] ? coverage.each_cons(2).to_a : ProductionTextRules.range(input["spec"]).each_cons(2).map { |left, right| [left["last"], right["first"]] }
    unless input["range"]
      ProductionTextRules.range(input["spec"]).each do |range|
        batch = drama["production"].fetch("seriesReviews", {})["#{range['first']}-#{range['last']}"]
        expected = accepted[(range["first"] - 1)..(range["last"] - 1)].map { |artifact| artifact.slice("episodeOrder", "sha256", "sourceDigest") }
        require!(batch && batch["basis"] == ScreenplayRules.digest(expected) && batch.dig("result", "accepted") == true, "production_series_batch_not_accepted")
      end
    end
    pair = pairs.find { |orders| !boundary_accepted?(drama, orders) }
    if pair
      result = boundary(drama, pair, arguments, meter, update)
      return result if result["pending"] || result["accepted"] == false
      return { "pending" => true, "reviewedBoundary" => pair }
    end
    return prior["result"] if prior && prior["basis"] == basis && prior["result"]
    summary = if input["range"]
                { "status" => "pass", "issues" => [], "summary" => "指定分集均通过完整视听验收，批内相邻集衔接已复核。" }
              else
                final_review(drama, accepted, coverage)
              end
    mandatory = summary["issues"].select { |issue| issue["level"] == "must" }
    result = { "accepted" => mandatory.empty?, "coverage" => coverage, "issues" => summary["issues"], "summary" => summary["summary"],
      "filmBasis" => basis, "verifiedEpisodes" => accepted.map { |artifact| artifact.slice("episodeOrder", "sha256", "sourceDigest") } }
    unless mandatory.empty?
      order = mandatory[0]["episodeOrder"]
      target = input.fetch("repairTargets", []).find { |entry| entry["stage"] == "write_episode" && entry["episodeOrder"] == order }
      require!(target, "production_series_repair_target_missing")
      result["reworkTarget"] = target["id"]
    end
    update.call { |current| (current["production"]["seriesReviews"] ||= {})[key] = { "basis" => basis, "result" => result } }
    result
  end

  private

  def require!(condition, code)
    ProductionTextRules.require!(condition, code)
  end

  def boundary_key(drama, pair)
    artifacts = pair.map { |order| @film.accepted(drama, order).fetch("artifact") }
    ScreenplayRules.digest(artifacts.map { |artifact| artifact.slice("episodeOrder", "sha256", "sourceDigest") })
  end

  def boundary_accepted?(drama, pair)
    record = drama["production"].fetch("boundaryAudits", {})[boundary_key(drama, pair)]
    record && record.dig("result", "accepted") == true && record["file"] && File.file?(record["file"]["path"]) && Digest::SHA256.file(record["file"]["path"]).hexdigest == record["file"]["sha256"]
  end

  def boundary(drama, pair, arguments, meter, update)
    key = boundary_key(drama, pair)
    record = drama["production"].fetch("boundaryAudits", {})[key] || { "episodes" => pair }
    left, right = pair.map { |order| @film.accepted(drama, order).fetch("artifact") }
    file = File.join(production_media_root, "production-boundary-#{key[0, 32]}.mp4")
    if record["file"]
      require!(File.file?(file) && Digest::SHA256.file(file).hexdigest == record["file"]["sha256"], "production_boundary_file_changed")
    else
      create_boundary(left, right, file)
      record["file"] = ProductionSnapshot.proof(file)
      update.call { |current| (current["production"]["boundaryAudits"] ||= {})[key] = record }
    end
    return record["result"] if record["result"]
    episodes = pair.map { |order| drama["episodes"].find { |episode| episode["order"] == order } }
    if !record["visual"]
      context = { "boundary" => pair, "leftEnding" => episodes[0]["shots"].last.slice("summary", "actionEnd", "dialogue"),
        "rightOpening" => episodes[1]["shots"].first.slice("summary", "actionStart", "dialogue"),
        "instruction" => "这是左集最后三秒与右集开头三秒。按原剧本可以切场或时间跳跃，检查人物身份、道具、因果及预定情绪是否连续；结尾悬念不能误判为未交代。" }
      visual = @visual.review_source(drama: drama, path: file, context: context, identity: "boundary:#{key}", update: update)
      return visual if visual["pending"]
      record["visual"] = visual
      update.call { |current| current["production"]["boundaryAudits"][key] = record }
      return { "pending" => true, "step" => "boundary_audio", "boundary" => pair }
    end
    audio = @audio.review_source(drama: drama, path: file, context: { "instrumental" => false,
      "checks" => "这是相邻两集的尾三秒与首三秒，允许按剧本剪辑边界截取词句；检查非剧情要求的突兀音量、爆音、长静音与角色音色变化，不把取样截断的词句误报为整集漏词。" },
      kind: "boundary", identity: key, meter: meter, update: update)
    return audio if audio["pending"]
    require!(boundary_key(drama, pair) == key, "production_boundary_source_changed")
    issues = record["visual"]["issues"] + audio["issues"]
    accepted = issues.none? { |issue| issue["level"] == "must" }
    result = { "accepted" => accepted, "issues" => issues, "boundary" => pair, "sourceDigest" => key, "heardAudio" => true, "viewedFrames" => true }
    unless accepted
      target = arguments["input"].fetch("repairTargets", []).find { |entry| entry["stage"] == "generate_video" && entry["episodeOrder"] == pair.last }
      require!(target, "production_boundary_repair_target_missing")
      result["reworkTarget"] = target["id"]
    end
    record.merge!("audio" => audio, "result" => result)
    update.call do |current|
      current["production"]["boundaryAudits"][key] = record
      unless accepted
        translated = issues.select { |issue| issue["level"] == "must" }.map { |issue| issue.merge("firstMs" => 0, "lastMs" => BOUNDARY_MS) }
        @film.request_repairs(current, episodes[1], right, translated)
      end
    end
    result
  end

  def create_boundary(left, right, output)
    FileUtils.mkdir_p(production_media_root)
    ending = [left["durationMs"] - BOUNDARY_MS, 0].max / 1000.0
    FFmpegTool.run!([FFmpegTool.find("ffmpeg"), "-v", "error", "-y", "-ss", format("%.3f", ending), "-t", "3", "-i", left["filePath"], "-t", "3", "-i", right["filePath"],
      "-filter_complex", "[0:v]setpts=PTS-STARTPTS[v0];[1:v]setpts=PTS-STARTPTS[v1];[0:a]asetpts=PTS-STARTPTS[a0];[1:a]asetpts=PTS-STARTPTS[a1];[v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]",
      "-map", "[v]", "-map", "[a]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "22", "-c:a", "aac", "-movflags", "+faststart", output], timeout: 60)
  end

  def final_review(drama, artifacts, coverage)
    summaries = coverage.map do |order|
      episode = drama["episodes"].find { |entry| entry["order"] == order }
      film = drama["production"]["filmAudits"].values.reverse.find { |entry| entry["episodeOrder"] == order && entry.dig("result", "accepted") }
      visual = drama["production"]["visualAudits"].values.find { |entry| entry["sourceSHA256"] == film["artifactSHA256"] }
      { "order" => order, "summary" => episode["summary"][0, 80], "artifactSHA256" => film["artifactSHA256"],
        "visualObservations" => visual ? visual["parts"].map { |part| part["summary"][0, 100] } : [],
        "heardTranscriptSHA256" => ScreenplayRules.digest(film["audio"]["heardTranscript"]), "accepted" => true }
    end
    @text.checked("全剧成片独立验收", '这是已完整逐集听音、看画面并逐一检查相邻集衔接后的整季验收。请依据已核验证据、结局与伏笔事实检查整季是否闭合；不是重新猜测或挑口味。输出 {status:pass|warn|block,summary,coverage,issues:[{rule,level:must|advice,episodeOrder,artifactSHA256,reason,suggestion}]}。coverage必须恰好等于expectedCoverage，问题必须引用对应集的真实artifactSHA256。缺证据不能通过。',
      { "expectedCoverage" => coverage, "plan" => drama["production"]["plan"], "films" => summaries,
        "verifiedBoundaryCount" => drama["episodeCount"] - 1, "batchAcceptances" => ProductionTextRules.range(drama["production"]["spec"]) }, writing: false) do |value|
      require!(value.is_a?(Hash) && %w[pass warn block].include?(value["status"]) && value["coverage"] == coverage, "production_series_review_invalid")
      ProductionTextRules.text!(value["summary"], "series acceptance summary", 1000)
      require!(value["issues"].is_a?(Array) && value["issues"].length <= 30, "production_series_issues_invalid")
      value["issues"].each do |issue|
        artifact = artifacts.find { |entry| entry["episodeOrder"] == issue["episodeOrder"] }
        require!(artifact && artifact["sha256"] == issue["artifactSHA256"] && %w[must advice].include?(issue["level"]), "production_series_issue_evidence_invalid")
        %w[rule reason suggestion].each { |key| ProductionTextRules.text!(issue[key], key, 1000) }
      end
      require!(value["status"] != "block" || value["issues"].any? { |issue| issue["level"] == "must" }, "production_series_block_without_issue")
      require!(value["status"] != "pass" || value["issues"].none? { |issue| issue["level"] == "must" }, "production_series_status_conflict")
    end
  end
end

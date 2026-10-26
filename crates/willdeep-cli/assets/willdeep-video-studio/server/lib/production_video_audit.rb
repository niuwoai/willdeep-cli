# frozen_string_literal: true

require_relative "production_visual_audit"
require_relative "production_audio_audit"
require_relative "qa_remediation"

class ProductionVideoAudit
  def initialize(videos:, video_store:, dramas:, visual:, audio:, reviews:)
    @videos, @store, @dramas, @visual, @audio, @reviews = videos, video_store, dramas, visual, audio, reviews
  end

  def inputs_digest(drama, item)
    _episode, shot = locate(drama, item)
    media = %w[start end].map do |kind|
      candidate = Array(shot["#{kind}Candidates"]).find { |entry| entry["id"] == shot[kind == "start" ? "selectedStartID" : "selectedEndID"] }
      media_proof(candidate)
    end
    Array(shot.dig("package", "cast")).each do |entry|
      character = drama["characters"].find { |value| value["id"] == entry["characterID"] }
      media << media_proof(character && MediaRef.selected(character, @dramas.media_root))
    end
    ReferencePackage.referenced_asset_ids(shot).each do |id|
      asset = drama["assets"].find { |entry| entry["id"] == id }
      next if asset && asset["kind"] == "voice"
      media << media_proof(asset && MediaRef.selected(asset, @dramas.media_root))
    end
    shot["dialogue"].each { |line| media << media_proof(@audio.line_source(line)) }
    ScreenplayRules.digest([shot.slice("id", "order", "duration", "summary", "dialogue", "package", "actionStart", "actionEnd", "cameraIntent", "soundscape", "music", "startPrompt", "endPrompt"), media, drama["visualStyle"]])
  end

  def reusable(drama, item)
    digest = inputs_digest(drama, item)
    drama["production"].fetch("videoAudits", {}).values.reverse.find do |record|
      result = record["result"]
      job = result && @store.find_job(result["jobID"])
      repair = result && drama["production"].fetch("filmRepairs", {}).values.any? { |mark| mark["shotID"] == item["shotID"] && mark["sourceSHA256"] == result["sha256"] }
      result && result["accepted"] && !repair && job && job["deletedAt"].to_s.empty? && record["shotID"] == item["shotID"] && record["inputsDigest"] == digest && valid_artifact?(result)
    end&.fetch("result")
  end

  def review(drama:, item:, meter:, update:)
    episode, shot = locate(drama, item)
    generation = generated(drama, episode, item)
    return failure("video_generation_missing", "本镜没有可验证的已结算视频。") unless generation && valid_artifact?(generation)
    job = @store.find_job(generation["jobID"])
    require!(job && job["shotID"] == shot["id"] && job["dramaID"] == drama["id"], "production_video_job_mismatch")
    digest = inputs_digest(drama, item)
    key = ScreenplayRules.digest([shot["id"], generation["sha256"], digest])
    record = drama["production"].fetch("videoAudits", {})[key] || { "shotID" => shot["id"], "inputsDigest" => digest, "jobID" => job["id"] }
    return record["result"] if record["result"]
    text = shot["dialogue"].map { |line| line["text"] }.join
    unless record["visual"]
      visual = @visual.review_source(drama: drama, path: generation["filePath"], context: shot.slice("summary", "dialogue", "actionStart", "actionEnd", "cameraIntent", "soundscape", "music"),
        identity: "video:#{shot['id']}:#{job['id']}", character_ids: Array(shot.dig("package", "cast")).map { |cast| cast["characterID"] }, update: update)
      return visual if visual["pending"]
      record["visual"] = visual
      update.call { |current| (current["production"]["videoAudits"] ||= {})[key] = record }
      return { "pending" => true, "step" => "audio" }
    end
    audio = @audio.review_source(drama: drama, path: generation["filePath"], context: { "expectedText" => text, "instrumental" => false, "forbidSpeech" => text.empty? },
      kind: "video", identity: "video:#{shot['id']}:#{job['id']}", meter: meter, update: update)
    return audio if audio["pending"]
    require!(inputs_digest(drama, item) == digest && valid_artifact?(generation), "production_video_audit_stale")
    issues = record["visual"]["issues"] + audio["issues"]
    info = FFmpegTool.probe(generation["filePath"])
    frame = @dramas.frame(drama)["video"]
    ratio = frame["width"].to_f / frame["height"]
    if (info["width"].to_f / info["height"] - ratio).abs > 0.015 || (info["durationMs"].to_i - shot["duration"].to_i * 1000).abs > 500
      issues << { "rule" => "framing", "level" => "must", "reason" => "实际画幅或时长与本镜约定不符。", "suggestion" => "按已定画幅和镜头秒数重新生成。", "firstMs" => 0, "lastMs" => info["durationMs"] }
    end
    accepted = issues.none? { |issue| issue["level"] == "must" }
    result = generation.merge("accepted" => accepted, "issues" => issues, "viewedFrames" => true, "heardAudio" => audio["heardAudio"],
      "visualCoverage" => record["visual"]["coverage"], "audioCoverage" => audio["coverage"], "shotID" => shot["id"], "inputsDigest" => digest)
    persist_legacy_review(job, issues, record["visual"])
    record.merge!("audio" => audio, "result" => result)
    update.call do |current|
      (current["production"]["videoAudits"] ||= {})[key] = record
      if accepted
        current_shot = current["episodes"].find { |entry| entry["id"] == episode["id"] }["shots"].find { |entry| entry["id"] == shot["id"] }
        require!(current_shot["selectedVideoID"] == shot["selectedVideoID"] || current_shot["selectedVideoID"] == job["id"], "production_user_video_selection_changed")
        current_shot.merge!("selectedVideoID" => job["id"], "selectedVideoAt" => Time.now.utc.iso8601, "selectedVideoBy" => "batch")
      end
    end
    result
  end

  def repair_notes(drama, item)
    records = drama["production"].fetch("videoAudits", {}).values.select { |record| record["shotID"] == item["shotID"] && record.dig("result", "accepted") == false }
    issues = records.last.to_h.dig("result", "issues") || []
    issues += drama["production"].fetch("filmRepairs", {}).values.select { |mark| mark["shotID"] == item["shotID"] }.flat_map { |mark| mark["issues"] }
    notes = issues.select { |issue| issue["level"] == "must" }.flat_map { |issue| QARemediation.remediations(issue["rule"]).first(1) }
    if issues.any? { |issue| issue["rule"].to_s.start_with?("dialogue_") }
      notes << { "id" => "production/dialogue", "text" => "角色逐字说出给定对白，音轨内容与给定台词对应，表情和嘴部动作自然同步", "category" => "dialogue" }
    end
    QARemediation.dedupe(notes).reject { |note| QARemediation.negative?(note["text"]) }.first(VideoService::MAX_EXTRA_DIRECTIVES)
  end

  private

  def require!(condition, code)
    ProductionTextRules.require!(condition, code)
  end

  def locate(drama, item)
    episode = drama["episodes"].find { |entry| entry["id"] == item["episodeID"] }
    shot = episode && episode["shots"].find { |entry| entry["id"] == item["shotID"] }
    require!(shot, "production_shot_missing")
    [episode, shot]
  end

  def media_proof(candidate)
    require!(candidate, "production_video_input_missing")
    path = candidate["filePath"] || MediaRef.path(candidate, @dramas.media_root)
    require!(File.file?(path.to_s), "production_video_input_missing")
    { "id" => candidate["id"], "sha256" => Digest::SHA256.file(path).hexdigest }
  end

  def valid_artifact?(artifact)
    path = artifact["filePath"]
    File.file?(path.to_s) && File.size(path) == artifact["bytes"] && Digest::SHA256.file(path).hexdigest == artifact["sha256"]
  end

  def generated(drama, episode, item)
    steps = drama["production"].fetch("mediaSteps", {}).values.select { |step| step["stage"] == "generate_video" && step["episodeOrder"] == episode["order"] }
    steps.sort_by { |step| step["attempt"].to_i }.reverse.map { |step| step.dig("items", ScreenplayRules.digest(item)) }.compact.first
  end

  def failure(rule, reason)
    { "accepted" => false, "issues" => [{ "rule" => rule, "level" => "must", "reason" => reason, "suggestion" => "补齐本镜视频并重新审核。", "firstMs" => 0, "lastMs" => 0 }] }
  end

  def persist_legacy_review(job, issues, visual)
    review = { "status" => issues.any? { |issue| issue["level"] == "must" } ? "block" : issues.empty? ? "pass" : "warn", "summary" => "自动制作画面与声音审核；完整证据保存在制作记录。",
      "issues" => issues.first(50).map do |issue|
        { "category" => ProductionVisualAudit::CATEGORIES.include?(issue["rule"]) ? issue["rule"] : "dialogue", "level" => issue["level"],
          "severity" => issue["level"] == "must" ? "block" : "warn", "reason" => issue["reason"], "suggestion" => issue["suggestion"], "location" => "#{issue['firstMs']}ms-#{issue['lastMs']}ms" }
      end }
    result = @videos.record_review("id" => job["id"], "aspect" => "frames", "review" => review, "basis" => @reviews.frames_basis(job), "model" => "production-audiovisual", "mediaImages" => visual["attachedImages"], "mediaVideos" => 0)
    require!(result["ok"], "production_video_review_write_failed")
  end
end

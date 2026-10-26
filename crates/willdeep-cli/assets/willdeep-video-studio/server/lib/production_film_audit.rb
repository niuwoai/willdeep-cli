# frozen_string_literal: true

require_relative "production_composition"

class ProductionFilmAudit
  def initialize(visual:, audio:, composition:)
    @visual, @audio, @composition = visual, audio, composition
  end

  def current(drama, order)
    record = drama["production"].fetch("compositions", {}).values.reverse.find { |entry| entry["episodeOrder"] == order && entry["result"] }
    require!(record && record["snapshot"], "production_film_composition_missing")
    snapshot = record["snapshot"]
    ProductionSnapshot.verify!(snapshot)
    episode = drama["episodes"].find { |entry| entry["order"] == order }
    args = snapshot.slice("workflowID", "taskID", "inputDigest")
    latest = @composition.build_snapshot(drama, episode, args)
    require!(latest["sourceDigest"] == record["sourceDigest"], "production_film_source_changed")
    artifact = record["result"]["artifact"]
    proof = ProductionSnapshot.proof(artifact["filePath"])
    require!(proof["sha256"] == artifact["sha256"] && proof["bytes"] == artifact["bytes"], "production_film_file_changed")
    [episode, artifact]
  end

  def accepted(drama, order)
    _episode, artifact = current(drama, order)
    drama["production"].fetch("filmAudits", {}).values.reverse.find do |record|
      record["episodeOrder"] == order && record["artifactSHA256"] == artifact["sha256"] && record["sourceDigest"] == artifact["sourceDigest"] && record.dig("result", "accepted") == true
    end&.fetch("result")
  end

  def request_repairs(drama, episode, artifact, issues)
    mark_repairs(drama, episode, artifact, issues)
  end

  def review(drama:, item:, arguments:, meter:, update:)
    episode, artifact = current(drama, item["episodeOrder"])
    key = ScreenplayRules.digest([episode["id"], artifact["sha256"], artifact["sourceDigest"]])
    record = drama["production"].fetch("filmAudits", {})[key] || { "episodeOrder" => episode["order"], "artifactSHA256" => artifact["sha256"], "sourceDigest" => artifact["sourceDigest"] }
    return record["result"] if record["result"]
    unless record["decoded"]
      FFmpegTool.run!([FFmpegTool.find("ffmpeg"), "-v", "error", "-xerror", "-i", artifact["filePath"], "-map", "0:v:0", "-map", "0:a:0", "-f", "null", "-"], timeout: 120)
      record["decoded"] = true
    end
    unless record["visual"]
      context = episode.slice("order", "title", "summary", "script").merge("shots" => episode["shots"].map { |shot| shot.slice("order", "summary", "cameraIntent", "duration") })
      output = @visual.review_source(drama: drama, path: artifact["filePath"], context: context, identity: "film:#{episode['id']}:#{artifact['sha256']}", update: update)
      return output if output["pending"]
      record["visual"] = output
      update.call { |current| (current["production"]["filmAudits"] ||= {})[key] = record }
      return { "pending" => true, "step" => "final_audio" }
    end
    transcript = episode["shots"].sort_by { |shot| shot["order"] }.flat_map { |shot| shot["dialogue"].map { |line| line["text"] } }.join
    output = @audio.review_source(drama: drama, path: artifact["filePath"], context: { "expectedText" => transcript, "instrumental" => false,
      "checks" => "这是最终混音。逐字检查所有对白完整、清楚，配乐情绪匹配且不掩盖对白，转场音量自然，没有额外人声、静音或爆音。" }, kind: "film", identity: episode["id"], meter: meter, update: update)
    return output if output["pending"]
    current(drama, episode["order"])
    visual_issues = record["visual"]["issues"]
    issues = visual_issues + output["issues"]
    accepted = issues.none? { |issue| issue["level"] == "must" }
    target_stage = visual_issues.any? { |issue| issue["level"] == "must" } ? "generate_video" : "compose_episode"
    target = arguments["input"].fetch("repairTargets", []).find { |entry| entry["stage"] == target_stage && entry["episodeOrder"] == episode["order"] }
    require!(accepted || target, "production_film_repair_target_missing")
    result = { "accepted" => accepted, "artifact" => artifact, "issues" => issues, "viewedFrames" => true, "heardAudio" => true,
      "visualCoverage" => record["visual"]["coverage"], "audioCoverage" => output["coverage"], "decoded" => true }
    result["reworkTarget"] = target["id"] unless accepted
    record.merge!("audio" => output, "result" => result)
    update.call do |current|
      (current["production"]["filmAudits"] ||= {})[key] = record
      mark_repairs(current, episode, artifact, visual_issues) unless accepted
    end
    result
  end

  private

  def require!(condition, code)
    ProductionTextRules.require!(condition, code)
  end

  def mark_repairs(drama, episode, artifact, issues)
    cursor = 0
    episode["shots"].sort_by { |shot| shot["order"] }.each do |shot|
      ending = cursor + shot["duration"] * 1000
      relevant = issues.select { |issue| issue["level"] == "must" && issue["firstMs"] < ending && issue["lastMs"] > cursor }
      unless relevant.empty?
        audit = drama["production"]["videoAudits"].values.reverse.find { |record| record["shotID"] == shot["id"] && record.dig("result", "jobID") == shot["selectedVideoID"] }
        require!(audit, "production_film_shot_provenance_missing")
        id = ScreenplayRules.digest([artifact["sha256"], shot["id"]])
        (drama["production"]["filmRepairs"] ||= {})[id] = { "shotID" => shot["id"], "sourceSHA256" => audit["result"]["sha256"], "issues" => relevant }
      end
      cursor = ending
    end
  end
end

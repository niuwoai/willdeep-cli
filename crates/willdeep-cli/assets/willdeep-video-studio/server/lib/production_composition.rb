# frozen_string_literal: true

require_relative "production_snapshot"
require_relative "episode_plan"

class ProductionComposition
  def initialize(dramas:, videos:, composer:, compose_store:, video_audit:, audio:)
    @dramas, @videos, @composer, @store, @video, @audio = dramas, videos, composer, compose_store, video_audit, audio
  end

  def step(drama:, item:, arguments:, update:)
    episode = drama["episodes"].find { |entry| entry["order"] == item["episodeOrder"] }
    require!(episode, "production_episode_missing")
    key = "#{arguments['taskID']}:#{arguments['input']['attempt']}"
    record = drama["production"].fetch("compositions", {})[key]
    snapshot = build_snapshot(drama, episode, arguments)
    if record
      require!(record["sourceDigest"] == snapshot["sourceDigest"], "production_composition_input_changed")
      return record["result"] if record["result"] && valid_artifact?(record["result"]["artifact"])
    else
      record = { "sourceDigest" => snapshot["sourceDigest"], "snapshot" => snapshot, "episodeOrder" => episode["order"], "jobs" => [] }
      update.call { |current| (current["production"]["compositions"] ||= {})[key] = record }
    end
    job = record["jobs"].empty? ? nil : @composer.status("jobID" => record["jobs"].last).fetch("jobs").first
    if job && %w[failed canceled interrupted].include?(job["state"])
      require!(record["jobs"].length < 3, "production_composition_retry_limit")
      job = nil
    end
    unless job
      token = "production:#{key}:compose:#{record['jobs'].length + 1}"
      submitted = @composer.compose({ "dramaID" => drama["id"], "episodeID" => episode["id"], "requestID" => token }, production: snapshot)
      require!(submitted["ok"] && submitted["job"], "production_composition_start_failed")
      job = submitted["job"]
      require!(job["productionSourceDigest"] == snapshot["sourceDigest"], "production_composition_job_mismatch")
      record["jobs"] << job["id"] unless record["jobs"].include?(job["id"])
      update.call { |current| current["production"]["compositions"][key] = record }
    end
    return { "pending" => true, "composeJobID" => job["id"], "progress" => job["progress"], "step" => job["step"] } unless job["state"] == "completed"
    path = job["outputPath"]
    require!(File.file?(path.to_s), "production_composition_file_missing")
    info = FFmpegTool.probe(path)
    require!(info && info["hasVideo"] && info["hasAudio"] && (info["durationMs"] - snapshot["expectedDurationMs"]).abs <= 500, "production_composition_output_invalid")
    file = ProductionSnapshot.proof(path)
    result = { "artifact" => { "episodeOrder" => episode["order"], "filePath" => file["path"], "sha256" => file["sha256"], "bytes" => file["bytes"],
      "durationMs" => info["durationMs"], "jobID" => job["id"], "sourceDigest" => snapshot["sourceDigest"], "playbackURL" => job["playbackURL"] } }
    record["result"] = result
    update.call { |current| current["production"]["compositions"][key] = record }
    result
  end

  def build_snapshot(drama, episode, arguments)
    files = []
    shots = episode["shots"].sort_by { |shot| shot["order"] }
    shots.each do |shot|
      item = { "episodeID" => episode["id"], "shotID" => shot["id"] }
      accepted = @video.reusable(drama, item)
      require!(accepted && accepted["jobID"] == shot["selectedVideoID"], "production_composition_video_not_accepted")
      files << ProductionSnapshot.proof(accepted["filePath"])
      shot["dialogue"].each do |line|
        require!(@audio.accepted_line?(drama, line), "production_composition_dialogue_not_accepted")
        source = @audio.line_source(line)
        character = drama["characters"].find { |entry| entry["name"] == line["speaker"] }
        bound = Array(shot.dig("package", "cast")).find { |entry| entry["characterID"] == character["id"] }
        require!(bound && source["voiceAssetID"] == bound["voiceID"], "production_composition_voice_changed")
        files << ProductionSnapshot.proof(source["filePath"])
      end
    end
    settings = @store.settings(drama["id"], episode["id"])
    settings.merge!("voiceSource" => "tts", "pacing" => "picture", "shotPacing" => {}, "originalVolume" => 0.0,
      "shotVoiceSource" => shots.select { |shot| shot["dialogue"].empty? }.to_h { |shot| [shot["id"], "video"] })
    bgm = settings["bgm"]
    previous_film = drama["production"].fetch("filmAudits", {}).values.reverse.find { |record| record["episodeOrder"] == episode["order"] && record.dig("result", "accepted") == false }
    if previous_film && previous_film.dig("result", "issues").any? { |issue| issue["rule"].to_s.start_with?("dialogue_") }
      bgm["volume"] = [bgm["volume"].to_f, 0.08].min
    end
    require!(bgm["source"] == "file" && bgm["volume"].to_f.positive? && MediaRef::NAME_PATTERN.match?(bgm["fileName"].to_s), "production_composition_music_missing")
    music = File.join(@dramas.media_root, bgm["fileName"])
    music_proof = ProductionSnapshot.proof(music)
    require!(drama["production"].fetch("audioAudits", {}).values.any? { |audit| audit["kind"] == "music" && audit["sourceSHA256"] == music_proof["sha256"] && audit.dig("result", "accepted") == true }, "production_composition_music_not_accepted")
    files << music_proof
    plan = EpisodePlan.build(drama: drama, episode: episode, jobs: @videos.jobs_with_media, settings: settings, media_root: @dramas.media_root)
    require!(plan["blocking"].empty? && plan["shots"].all? { |shot| !shot["dialogueStale"] && !shot["qaOverride"] }, "production_composition_plan_blocked")
    plan["shots"].each do |shot|
      require!(shot["voiceSource"] != "tts" || shot["dialogueTrack"] || shot["lines"].all? { |line| line["audio"] == "ready" }, "production_composition_audio_missing")
      files << ProductionSnapshot.proof(shot["dialogueTrack"]["filePath"]) if shot["dialogueTrack"]
    end
    frame = @dramas.frame(drama)
    snapshot = { "version" => 1, "workflowID" => arguments["workflowID"], "taskID" => arguments["taskID"], "inputDigest" => arguments["inputDigest"],
      "episodeOrder" => episode["order"], "expectedDurationMs" => drama["production"]["spec"]["episode_seconds"] * 1000,
      "drama" => drama.slice("id", "title", "aspectRatio", "visualStyle").merge("frame" => frame), "episode" => episode.slice("id", "order", "title"),
      "plan" => plan.merge("settings" => settings), "bgmPath" => music_proof["path"], "files" => files.uniq { |file| file["path"] } }
    snapshot["sourceDigest"] = ProductionSnapshot.digest(snapshot)
    snapshot
  end

  private

  def require!(condition, code)
    ProductionTextRules.require!(condition, code)
  end

  def valid_artifact?(artifact)
    artifact && File.file?(artifact["filePath"].to_s) && File.size(artifact["filePath"]) == artifact["bytes"] && Digest::SHA256.file(artifact["filePath"]).hexdigest == artifact["sha256"]
  end
end

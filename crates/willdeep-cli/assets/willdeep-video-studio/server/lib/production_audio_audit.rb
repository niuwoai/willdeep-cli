# frozen_string_literal: true

require_relative "production_audio_backend"
require_relative "production_media_root"
require_relative "ffmpeg_tool"
require_relative "media_ref"
require_relative "review_material"
require_relative "reference_package"
require "fileutils"

class ProductionAudioAudit
  include ProductionMediaRoot
  RUBRIC = "production-audio-v1"
  def initialize(store:, media_root:, backend:, env: ENV)
    @store, @root, @backend, @env = store, media_root, backend, env
  end

  def available?
    !@backend.call.nil? && FFmpegTool.available?(env: @env)
  rescue ProductionAudioBackend::Error
    false
  end

  def review(drama:, item:, stage:, meter:, update:)
    episode = drama["episodes"].find { |entry| entry["id"] == item["episodeID"] || entry["order"] == item["episodeOrder"] }
    require!(episode, "production_episode_missing")
    if stage == "review_dialogue"
      shot = episode["shots"].find { |entry| entry["id"] == item["shotID"] }
      line = shot && shot["dialogue"].find { |entry| entry["id"] == item["lineID"] }
      source = line_source(line)
      return local_failure("dialogue_audio_missing_or_stale", "当前台词没有可核验的配音文件。") unless source
      context = { "expectedText" => line["text"], "speaker" => line["speaker"], "instrumental" => false }
      output = review_source(drama: drama, path: source["filePath"], context: context, kind: "dialogue", meter: meter, identity: item["lineID"], update: update)
      return output if output["pending"]
      audio_ms = shot["dialogue"].sum { |entry| current = line_source(entry); current ? current["durationMs"].to_i : 0 }
      audio_ms += [shot["dialogue"].length - 1, 0].max * ReferencePackage::DIALOGUE_GAP_MS
      if audio_ms > shot["duration"].to_i * 1000
        output = output.merge("accepted" => false, "issues" => output["issues"] + [issue("dialogue_too_long", "本镜对白音轨超过已定分镜时长。", "保持原台词，调整语速或重新拆分分镜。", 0, output["durationMs"])])
      end
      output.merge("candidateID" => source["id"], "lineID" => line["id"])
    else
      steps = drama["production"].fetch("mediaSteps", {}).values.select { |step| step["stage"] == "generate_music" && step["episodeOrder"] == episode["order"] }
      generated = steps.max_by { |step| step["attempt"].to_i }&.fetch("items", {})&.values&.first
      return local_failure("music_missing", "没有本集已结算的音乐文件。") unless generated && File.file?(generated["filePath"].to_s)
      context = { "expectedText" => "", "instrumental" => true, "musicPrompt" => drama["production"]["storyboards"][episode["order"].to_s]["musicPrompt"] }
      review_source(drama: drama, path: generated["filePath"], context: context, kind: "music", meter: meter, identity: episode["id"], update: update)
    end
  end

  def line_source(line)
    return nil unless line && line["audio"].is_a?(Hash)
    candidate = Array(line["audio"]["candidates"]).find { |entry| entry["id"] == line["audio"]["selectedCandidateID"] }
    return nil unless candidate && candidate["textFingerprint"] == ReviewMaterial.fingerprint(line["text"])
    path = MediaRef.path(candidate, production_media_root)
    return nil unless path && File.file?(path)
    candidate.merge("filePath" => path)
  end

  def accepted_line?(drama, line)
    source = line_source(line)
    return false unless source
    sha = Digest::SHA256.file(source["filePath"]).hexdigest
    review = drama["production"].fetch("mediaSteps", {}).values.select { |step| step["stage"] == "review_dialogue" }
      .flat_map { |step| step.fetch("items", {}).values }.select { |result| result["lineID"] == line["id"] && result["candidateID"] == source["id"] }.last
    return false if review && review["accepted"] == false
    drama["production"].fetch("audioAudits", {}).values.any? do |record|
      record["kind"] == "dialogue" && record["identity"] == line["id"] && record["sourceSHA256"] == sha &&
        record.dig("result", "accepted") == true
    end
  end

  def corrections(drama, line)
    drama["production"].fetch("mediaSteps", {}).values.select { |step| step["stage"] == "review_dialogue" }
      .flat_map { |step| step.fetch("items", {}).values }.select { |result| result["lineID"] == line["id"] && result["accepted"] == false }
      .flat_map { |result| result.fetch("issues", []) }.last(20)
  end

  # Used for raw dialogue, full music, video audio, and the final mixed soundtrack.
  # Every consecutive segment is attached. A partial read cannot finish the item.
  def review_source(drama:, path:, context:, kind:, meter:, identity:, update:)
    backend = @backend.call
    require!(backend, "production_audio_review_unavailable")
    require!(File.file?(path.to_s), "production_audio_source_missing")
    info = FFmpegTool.probe(path, env: @env)
    return local_failure("audio_stream_missing", "文件中没有可读取的音轨。") unless info && info["hasAudio"] && info["durationMs"].to_i.positive?
    duration = info["durationMs"].to_i
    require!(duration <= 600_000, "production_audio_source_duration_invalid")
    sha = Digest::SHA256.file(path).hexdigest
    key = ScreenplayRules.digest([RUBRIC, kind, identity, sha, context])
    record = drama["production"].fetch("audioAudits", {})[key] || { "kind" => kind, "identity" => identity,
      "sourceSHA256" => sha, "context" => context, "durationMs" => duration, "parts" => [] }
    return record["result"] if record["result"]
    count = (duration / (ProductionAudioBackend::MAX_SECONDS * 1000.0)).ceil
    index = record["parts"].length
    first_ms = index * ProductionAudioBackend::MAX_SECONDS * 1000
    last_ms = [duration, first_ms + ProductionAudioBackend::MAX_SECONDS * 1000].min
    require!(index < count, "production_audio_coverage_invalid")
    root = production_media_root
    FileUtils.mkdir_p(root)
    segment = File.join(root, "audio-qa-#{key[0, 32]}-#{index + 1}.wav")
    extract(path, segment, first_ms, last_ms)
    answer = backend.judge(path: segment, material: context.merge("kind" => kind, "sourceFirstMs" => first_ms, "sourceLastMs" => last_ms,
      "sourceDurationMs" => duration, "sourceSHA256" => sha), meter: meter, identity: "audio:#{identity}:#{index + 1}")
    require!(Digest::SHA256.file(path).hexdigest == sha, "production_audio_source_changed")
    record["parts"] << answer.merge("sourceFirstMs" => first_ms, "sourceLastMs" => last_ms)
    if record["parts"].length == count
      record["result"] = finish(record, context)
    end
    update.call { |current| (current["production"]["audioAudits"] ||= {})[key] = record }
    record["result"] || { "pending" => true, "heardParts" => record["parts"].length, "totalParts" => count }
  end

  private

  def require!(condition, code)
    ProductionTextRules.require!(condition, code)
  end

  def extract(source, target, first_ms, last_ms)
    ffmpeg = FFmpegTool.find("ffmpeg", env: @env)
    require!(ffmpeg, "production_audio_ffmpeg_missing")
    FFmpegTool.run!([ffmpeg, "-v", "error", "-y", "-ss", format("%.3f", first_ms / 1000.0), "-i", source,
      "-t", format("%.3f", (last_ms - first_ms) / 1000.0), "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", target], timeout: 60)
  end

  def normalized(text)
    text.to_s.gsub(/[^\p{L}\p{N}]/u, "").downcase
  end

  def issue(rule, reason, suggestion, first, last)
    { "rule" => rule, "level" => "must", "reason" => reason, "suggestion" => suggestion, "firstMs" => first, "lastMs" => last }
  end

  def local_failure(rule, reason)
    { "accepted" => false, "heardAudio" => false, "issues" => [issue(rule, reason, "补齐有效音频后重新审核。", 0, 0)] }
  end

  def finish(record, context)
    parts = record["parts"]
    cursor = 0
    parts.each do |part|
      require!(part["heardAudio"] == true && part["sourceFirstMs"] == cursor && part["sourceLastMs"] > cursor, "production_audio_coverage_invalid")
      cursor = part["sourceLastMs"]
    end
    require!(cursor == record["durationMs"], "production_audio_coverage_incomplete")
    transcript = parts.map { |part| part["heardTranscript"] }.join
    issues = parts.flat_map do |part|
      value = part["issues"].map { |entry| entry.merge("firstMs" => entry["firstMs"] + part["sourceFirstMs"], "lastMs" => entry["lastMs"] + part["sourceFirstMs"]) }
      if part["status"] == "block" && value.none? { |entry| entry["level"] == "must" }
        value << issue("audio_blocked", part["summary"], "重新生成并复审此音频。", part["sourceFirstMs"], part["sourceLastMs"])
      end
      value
    end
    expected = context["expectedText"]
    if expected && !expected.empty? && normalized(transcript) != normalized(expected)
      issues << issue("dialogue_transcript_mismatch", "实际听到的对白与已定稿台词不一致。", "逐字保留原台词重新配音，修正漏词、错词或重复词。", 0, cursor)
    end
    if (context["instrumental"] || context["forbidSpeech"]) && (parts.any? { |part| part["hasSpeech"] } || !transcript.empty?)
      issues << issue(context["instrumental"] ? "music_contains_voice" : "unexpected_speech", "不应有人声的音轨中检测到人声。", "保持已定声音设计，重新生成对应音轨。", 0, cursor)
    end
    { "accepted" => issues.none? { |entry| entry["level"] == "must" }, "heardAudio" => true, "heardTranscript" => transcript,
      "sourceSHA256" => record["sourceSHA256"], "durationMs" => cursor, "issues" => issues,
      "coverage" => parts.map { |part| part.slice("sourceFirstMs", "sourceLastMs", "audioSHA256", "model") } }
  end
end

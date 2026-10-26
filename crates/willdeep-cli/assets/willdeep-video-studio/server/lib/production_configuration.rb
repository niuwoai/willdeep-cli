# frozen_string_literal: true

# Read-only setup report. Configuration presence is not a paid model probe.
# This report never returns endpoints, keys, stock voice IDs or provider bodies.
class ProductionConfiguration
  def initialize(audio:, voices:, compose:, video:)
    @audio, @voices, @compose, @video = audio, voices, compose, video
  end

  def call
    compose = @compose.call
    video = @video.call
    checks = { "audioReview" => @audio.call.slice("state"), "stockVoices" => @voices.configuration,
      "tts" => { "state" => compose["tts"] == true ? "configured" : "missing" },
      "music" => { "state" => compose["music"] != true ? "missing" : compose["musicReachable"] == true ? "available" : "unreachable" },
      "video" => { "state" => video["apiConfigured"] == true ? "configured" : "missing" },
      "ffmpeg" => { "state" => compose["ffmpeg"] == true ? "available" : "missing" } }
    { "ok" => true, "configurationOnly" => true, "checks" => checks }
  rescue StandardError
    { "ok" => false, "error" => { "code" => "production_configuration_failed" } }
  end
end

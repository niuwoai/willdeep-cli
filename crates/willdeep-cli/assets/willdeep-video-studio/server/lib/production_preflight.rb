# frozen_string_literal: true

class ProductionPreflight
  def initialize(host:, image:, compose:, video:, pipeline:)
    @host, @image, @compose, @video, @pipeline = host, image, compose, video, pipeline
  end

  def call(_spec)
    compose = @compose.call
    video = @video.call
    pipeline = @pipeline.call
    checks = {
      "model" => @host.supports?(HostBridge::AI_COMPLETE),
      "metering" => @host.supports?(ProductionMeter::METHOD),
      "image" => !@image.call.nil?, "tts" => compose["tts"] == true,
      "music" => compose["music"] == true && compose["musicReachable"] == true,
      "video" => video["apiConfigured"] == true, "ffmpeg" => compose["ffmpeg"] == true,
      "pipeline" => pipeline.is_a?(Hash) && pipeline["ready"] == true,
      "audioReview" => pipeline.is_a?(Hash) && pipeline["audioReview"] == true,
      "filmReview" => pipeline.is_a?(Hash) && pipeline["filmReview"] == true
    }
    checks["delivery"] = pipeline.is_a?(Hash) && pipeline["delivery"] == true
    checks["stockVoices"] = pipeline.is_a?(Hash) && pipeline["stockVoices"] == true
    { "ok" => checks.values.all?, "checks" => checks, "missing" => checks.select { |_name, ready| !ready }.keys }
  rescue StandardError => error
    { "ok" => false, "checks" => {}, "missing" => ["production_preflight_failed"], "errorClass" => error.class.name }
  end
end

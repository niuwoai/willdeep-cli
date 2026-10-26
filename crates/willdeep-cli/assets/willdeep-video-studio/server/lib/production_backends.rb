# frozen_string_literal: true

require_relative "production_meter"
require_relative "image_backend"

# These wrappers sit at the paid request, including retries. A selected host image
# backend is already metered by Runtime and must not be charged a second time here.
module ProductionBackends
  def self.provider(backend)
    "#{backend.class.name.gsub(/[^a-zA-Z0-9_.-]/, '_')}-#{backend.name}"[0, 120]
  end

  class Image
    def initialize(backend:, meter:, identity:)
      @backend, @meter, @identity = backend, meter, identity
    end

    def name
      @backend.name
    end

    def generate(**arguments)
      return @backend.generate(**arguments) if @backend.is_a?(ImageBackend::HostImageBackend)
      @meter.execute(kind: "image", provider: ProductionBackends.provider(@backend), identity: @identity,
        quantities: { "images" => 1 }, request: arguments.transform_keys(&:to_s)) do |_id|
        response = @backend.generate(**arguments)
        path = response.is_a?(Hash) && response["filePath"]
        raise ProductionMeter::Error, "production_image_file_missing" unless path && File.file?(path) && File.size(path).positive?
        { "response" => response, "actual" => { "images" => 1 } }
      end
    end
  end

  class Speech
    def initialize(backend:, meter:, identity:, adjustment: nil)
      @backend, @meter, @identity, @adjustment = backend, meter, identity, adjustment
    end

    def name
      @backend.name
    end

    def synthesize(**arguments)
      arguments = arguments.merge(@adjustment) if @adjustment
      @meter.execute(kind: "tts", provider: ProductionBackends.provider(@backend), identity: @identity,
        quantities: { "tts_characters" => arguments.fetch(:text).length }, request: arguments.transform_keys(&:to_s)) do |_id|
        response = @backend.synthesize(**arguments)
        path = response.is_a?(Hash) && response["filePath"]
        raise ProductionMeter::Error, "production_audio_file_missing" unless path && File.file?(path) && File.size(path).positive?
        raise ProductionMeter::Error, "production_audio_duration_missing" unless response["durationMs"].is_a?(Numeric) && response["durationMs"].positive?
        { "response" => response, "actual" => { "tts_characters" => arguments.fetch(:text).length } }
      end
    end
  end
end

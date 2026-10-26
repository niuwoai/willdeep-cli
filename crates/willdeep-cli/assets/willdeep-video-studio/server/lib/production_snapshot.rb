# frozen_string_literal: true

require "json"
require "digest"
require "fileutils"

module ProductionSnapshot
  extend self
  Error = Class.new(StandardError)

  def digest(value)
    Digest::SHA256.hexdigest(JSON.generate(value.reject { |key, _| key == "sourceDigest" }))
  end

  def proof(path)
    original = File.expand_path(path.to_s)
    canonical = File.realpath(original)
    raise Error, "production_snapshot_file_missing" unless File.file?(canonical) && File.size(canonical).positive?
    { "path" => canonical, "originalPath" => original, "bytes" => File.size(canonical), "sha256" => Digest::SHA256.file(canonical).hexdigest }
  end

  def validate!(snapshot)
    raise Error, "production_snapshot_invalid" unless snapshot.is_a?(Hash) && snapshot["version"] == 1 && snapshot["sourceDigest"] == digest(snapshot)
    raise Error, "production_snapshot_inputs_missing" unless snapshot["files"].is_a?(Array) && !snapshot["files"].empty?
    snapshot["files"].each do |file|
      raise Error, "production_snapshot_input_invalid" unless file.is_a?(Hash) && file["path"].is_a?(String) && File.absolute_path(file["path"]) == file["path"] &&
        file["bytes"].is_a?(Integer) && file["bytes"].positive? && file["sha256"].to_s.match?(/\A[0-9a-f]{64}\z/)
    end
    snapshot
  end

  def verify!(snapshot)
    validate!(snapshot)
    snapshot["files"].each do |file|
      raise Error, "production_snapshot_input_changed" unless File.file?(file["path"]) && File.size(file["path"]) == file["bytes"] && Digest::SHA256.file(file["path"]).hexdigest == file["sha256"]
    end
  end

  # Render against verified private copies. A source edited during copying is
  # rejected, and edits after the copy cannot change what the worker renders.
  def freeze_inputs(snapshot, directory)
    validate!(snapshot)
    mapping = {}
    snapshot["files"].each_with_index do |file, index|
      raise Error, "production_snapshot_duplicate_conflict" if mapping.key?(file["path"])
      extension = File.extname(file["path"]).gsub(/[^.a-zA-Z0-9]/, "")[0, 16]
      target = File.join(directory, "frozen-#{index}#{extension}")
      File.open(file["path"], "rb") { |input| File.open(target, "wb", 0o600) { |output| IO.copy_stream(input, output) } }
      raise Error, "production_snapshot_input_changed" unless File.size(target) == file["bytes"] && Digest::SHA256.file(target).hexdigest == file["sha256"]
      mapping[file["path"]] = target
      mapping[file["originalPath"]] = target if file["originalPath"]
    end
    rewrite(snapshot, mapping)
  rescue SystemCallError => error
    raise Error, "production_snapshot_io_failed: #{error.class}", cause: nil
  end

  def rewrite(value, mapping)
    case value
    when Hash then value.transform_values { |entry| rewrite(entry, mapping) }
    when Array then value.map { |entry| rewrite(entry, mapping) }
    when String then mapping.fetch(value, value)
    else value
    end
  end
end

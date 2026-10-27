# frozen_string_literal: true

require "digest"
require "json"

# Passed explicitly to a production adapter; never inherited through global/thread state.
# A resumed adapter queries a known upstream job or reuses a saved result. It never
# interprets a previously reserved request as permission to purchase it again.
class ProductionMeter
  METHOD = "willdeep/production/meter"
  UNITS = {
    "model" => %w[model_calls tokens],
    "image" => %w[images], "tts" => %w[tts_characters],
    "music" => %w[music_calls music_seconds], "video" => %w[video_calls video_seconds]
  }.freeze
  UUID = /\A[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\z/
  NIL_UUID = "00000000-0000-0000-0000-000000000000"
  IMAGE_BATCH_LIMIT = 1_536
  Error = Class.new(StandardError)

  def initialize(host:, task_id:, grant:, input_digest:, attempt:, image_policy: false)
    require!(task_id.is_a?(String) && UUID.match?(task_id), "production_task_id_invalid")
    require!(grant.is_a?(String) && UUID.match?(grant) && grant != NIL_UUID, "production_grant_invalid")
    require!(input_digest.is_a?(String) && input_digest.match?(/\A[0-9a-f]{64}\z/), "production_input_digest_invalid")
    require!(attempt.is_a?(Integer) && attempt.between?(1, image_policy ? IMAGE_BATCH_LIMIT : 4), "production_attempt_invalid")
    @host, @task_id, @grant, @input_digest, @attempt = host, task_id, grant, input_digest, attempt
  end

  def reserve(kind:, provider:, identity:, quantities:, request:, image_subject: nil)
    reference = reference(kind: kind, provider: provider, identity: identity, request: request)
    quantities!(kind, quantities, positive: true)
    input = { "request_id" => reference["requestID"], "node_id" => @task_id, "kind" => kind,
              "request_digest" => reference["requestDigest"], "provider" => provider, "quantities" => quantities }
    input["imageSubject"] = image_subject if image_subject
    answer = call("reserve", input)
    require!([true, false].include?(answer["fresh"]), "production_reservation_response_invalid")
    answer.merge("requestID" => reference["requestID"])
  end

  def image_status(subject)
    answer = call("image_status", "subject" => subject)
    require!(answer["limit"] == 4 && answer["attempts"].is_a?(Integer) && answer["attempts"] >= 0 &&
      [true, false].include?(answer["unresolved"]), "production_image_counter_invalid")
    require!(!answer["unresolved"], "production_uncertain_generation")
    answer
  end

  # Reconstruct an existing receipt's identity without asking for a reservation.
  def reference(kind:, provider:, identity:, request:)
    require!(UNITS.key?(kind), "production_generation_kind_invalid")
    require!(provider.is_a?(String) && provider.match?(/\A[a-zA-Z0-9_.-]{1,120}\z/), "production_provider_invalid")
    require!(identity.is_a?(String) && !identity.empty? && identity.length <= 512, "production_identity_invalid")
    digest = Digest::SHA256.hexdigest(JSON.generate(canonical(request)))
    { "requestID" => request_id(kind, provider, identity, digest), "requestDigest" => digest }
  end

  # Only a fresh reservation may enter the paid backend. The block must return
  # explicit physical quantities with its result; unknown metering is not zero.
  def execute(**arguments)
    reserved = reserve(**arguments)
    return reused(reserved) unless reserved["fresh"]

    id = reserved.fetch("requestID")
    begin
      result = yield id
      require!(result.is_a?(Hash) && result.key?("response"), "production_generation_response_invalid")
      quantities!(arguments.fetch(:kind), result["actual"], positive: false)
      call("settle", "requestID" => id, "actual" => result.fetch("actual"),
                     "response" => result.fetch("response"), "succeeded" => true)
      result.fetch("response")
    rescue StandardError => error
      begin
        call("unknown", "requestID" => id)
      rescue StandardError => reconciliation
        # No provider messages or credentials are printed. The original receipt remains reserved.
        raise Error, "production_reconciliation_failed: #{reconciliation.class}; original=#{error.class}"
      end
      raise Error, "production_generation_failed: #{error.class}", cause: nil
    end
  end

  def submitted(request_id, external_id)
    require!(external_id.is_a?(String) && !external_id.strip.empty?, "production_external_id_required")
    call("submitted", "requestID" => request_id, "externalID" => external_id)
  end

  def settle(request_id:, kind:, actual:, response:, succeeded:)
    quantities!(kind, actual, positive: false)
    require!([true, false].include?(succeeded), "production_outcome_required")
    call("settle", "requestID" => request_id, "actual" => actual,
                   "response" => response, "succeeded" => succeeded)
  end

  private

  def require!(condition, code)
    raise Error, code unless condition
  end

  def quantities!(kind, quantities, positive:)
    require!(quantities.is_a?(Hash) && quantities.keys.sort == UNITS.fetch(kind).sort &&
      quantities.values.all? { |value| value.is_a?(Integer) && value >= (positive ? 1 : 0) }, "production_quantities_invalid")
  end

  def call(operation, arguments)
    require!(@host.supports?(METHOD), "production_runtime_unsupported")
    result = @host.request(METHOD, { "taskID" => @task_id, "grant" => @grant,
                                    "operation" => operation, "arguments" => arguments })
    require!(result.is_a?(Hash) && result["ok"] == true, "production_meter_rejected")
    result
  end

  def reused(reservation)
    receipt = reservation["receipt"]
    require!(receipt.is_a?(Hash) && receipt["state"] == "settled" && receipt["succeeded"] == true &&
      reservation.key?("response"), "production_uncertain_generation")
    reservation.fetch("response")
  end

  def canonical(value)
    case value
    when Hash
      require!(value.keys.all? { |key| key.is_a?(String) }, "production_request_keys_invalid")
      value.keys.sort.each_with_object({}) { |key, result| result[key] = canonical(value[key]) }
    when Array then value.map { |item| canonical(item) }
    when Float
      require!(value.finite?, "production_request_value_invalid")
      value
    when String, Integer, TrueClass, FalseClass, NilClass then value
    else raise Error, "production_request_value_invalid"
    end
  end

  def request_id(kind, provider, identity, digest)
    namespace = [@task_id.delete("-")].pack("H*")
    name = JSON.generate([@input_digest, @attempt, kind, provider, identity, digest])
    bytes = Digest::SHA1.digest(namespace + name).bytes.first(16)
    bytes[6] = (bytes[6] & 0x0f) | 0x50
    bytes[8] = (bytes[8] & 0x3f) | 0x80
    hex = bytes.pack("C*").unpack("H*").first
    [hex[0, 8], hex[8, 4], hex[12, 4], hex[16, 4], hex[20, 12]].join("-")
  end
end

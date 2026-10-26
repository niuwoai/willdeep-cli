# frozen_string_literal: true

# This path has no submission, reservation, adoption or creative-model callbacks.
# It only closes receipts for jobs purchased before the user cancelled the work.
class ProductionReconciliation
  def initialize(store:, video_store:, endpoint:, music_poll:, video_poll:)
    @store, @video_store, @endpoint, @music_poll, @video_poll = store, video_store, endpoint, music_poll, video_poll
  end

  def call(drama:, arguments:, receipts:, meter:)
    prefix = "#{arguments['taskID']}:#{arguments['input']['attempt']}:"
    pending, unresolved, settled = false, [], []
    receipts.each do |receipt|
      require!(receipt["node_id"] == arguments["taskID"] && %w[music video].include?(receipt["kind"]), "production_reconciliation_scope_invalid")
      next if receipt["state"] == "settled"
      music = receipt["kind"] == "music"
      require!(arguments["input"]["stage"] == (music ? "generate_music" : "generate_video"), "production_reconciliation_scope_invalid")
      match = matching_record(drama, prefix, receipt, meter)
      if !match
        unresolved << receipt["request_id"]
        next
      end
      key, record = match
      require!(record["endpointDigest"] == @endpoint.call(music), "production_provider_configuration_changed")
      known, record = known_job(receipt, record, music)
      if !known
        unresolved << receipt["request_id"]
        next
      end
      record = record.merge("externalID" => known, "requestID" => receipt["request_id"])
      save_record(drama, key, record)
      meter.submitted(receipt["request_id"], known) unless receipt["external_id"]
      response = music ? @music_poll.call(known, record) : @video_poll.call(record)
      if response["pending"]
        pending = true
        next
      end
      require!(response["durationMs"].is_a?(Numeric) && response["durationMs"].positive?, "production_reconciliation_usage_unknown")
      save_record(drama, key, record.merge("response" => response))
      kind = receipt["kind"]
      meter.settle(request_id: receipt["request_id"], kind: kind,
        actual: { "#{kind}_calls" => 1, "#{kind}_seconds" => (response["durationMs"] / 1000.0).ceil }, response: response, succeeded: true)
      settled << receipt["request_id"]
    end
    { "settlementOnly" => true, "pending" => pending, "settledRequests" => settled, "unresolvedRequests" => unresolved }
  end

  private

  def require!(condition, code)
    ProductionTextRules.require!(condition, code)
  end

  def matching_record(drama, prefix, receipt, meter)
    drama["production"].fetch("remoteRequests", {}).find do |key, record|
      next false unless key.start_with?(prefix)
      reference = meter.reference(kind: receipt["kind"], provider: receipt["provider"], identity: key, request: record["request"])
      reference["requestID"] == receipt["request_id"] && reference["requestDigest"] == receipt["request_digest"]
    end
  end

  def known_job(receipt, record, music)
    identifiers = [record["externalID"], receipt["external_id"]].compact
    if !music && @video_store
      job = @video_store.jobs.find { |entry| entry["requestID"] == receipt["request_id"] }
      if job && !job["remoteID"].to_s.empty?
        identifiers << job["remoteID"]
        record = record.merge("jobID" => job["id"])
      end
    end
    require!(identifiers.all? { |value| value.is_a?(String) && !value.empty? } && identifiers.uniq.length <= 1, "production_external_id_conflict")
    [identifiers.first, record]
  end

  def save_record(drama, key, record)
    @store.update(drama["id"]) do |current|
      previous = current["production"].fetch("remoteRequests").fetch(key)
      require!(previous["request"] == record["request"] && previous["endpointDigest"] == record["endpointDigest"], "production_reconciliation_record_changed")
      current["production"]["remoteRequests"][key] = previous.merge(record)
    end
  end
end

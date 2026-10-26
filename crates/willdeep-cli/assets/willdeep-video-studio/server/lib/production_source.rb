# frozen_string_literal: true

require_relative "production_text"

# Original screenplay bytes/provenance remain immutable. After a repair, only
# this production's working candidates enter normal writing and review stages.
class ProductionSource
  def initialize(text:)
    @text = text
  end

  def prepare!(series, source)
    count = series["spec"]["episode_count"]
    require!(source["episodes"].map { |entry| entry["order"] } == (1..count).to_a && source["episodeDurationSeconds"] == series["spec"]["episode_seconds"], "production_source_spec_mismatch")
    canon = source.fetch("screenplay").slice("bible", "contracts", "threads")
    require!(canon["contracts"].is_a?(Array) && canon["contracts"].map { |entry| entry["order"] } == (1..count).to_a, "production_source_contracts_invalid")
    series["sourceCanon"] = copy(canon)
    series["plan"].merge!("ending" => canon.fetch("bible").fetch("ending"), "facts" => [], "threads" => copy(canon.fetch("threads")), "sourceBible" => copy(canon["bible"]))
    source["episodes"].each do |episode|
      order = episode["order"]
      raw = series.fetch("sourceCandidates").fetch(order.to_s)
      candidate = normalize(raw).merge("events" => [], "newFacts" => [], "reveals" => [], "threadEvents" => [])
      require!(ScreenplayRules.render(candidate) == episode["script"], "production_source_candidate_mismatch")
      contract = canon["contracts"].find { |entry| entry["order"] == order }
      series["outlines"][order.to_s] = copy(contract).merge(episode.slice("title", "summary")).merge("reveals" => [])
      require!(!series["episodes"].key?(order.to_s), "production_source_migration_conflict")
      series["episodes"][order.to_s] = { "candidate" => candidate, "adoptedBasis" => ScreenplayRules.digest(candidate),
        "history" => [], "reviews" => {}, "sourceInherited" => true, "metadataReady" => false }
    end
    series["sourceSnapshotDigest"] = snapshot_digest(series)
  end

  def verify!(drama)
    series = drama["production"]
    require!(series["sourceSnapshotDigest"] == snapshot_digest(series), "production_source_snapshot_changed")
    unless series["sourceDerivative"]
      current = drama["episodes"].map { |episode| episode.slice("order", "title", "summary", "script") }
      require!(current == series["sourceFrozen"]["scripts"], "production_source_snapshot_changed")
    end
  end

  def execute(drama, arguments)
    series, input = drama["production"], arguments["input"]
    verify!(drama)
    if !series["sourceDerivative"] && input["stage"] == "write_episode" && input["attempt"] > 1
      series["sourceDerivative"] = { "firstTaskID" => arguments["taskID"], "changedEpisodes" => [] }
      @text.invalidate_acceptance(drama)
    end
    return frozen_result(drama, input) unless series["sourceDerivative"]
    return @text.execute(drama, input) unless input["stage"] == "write_episode"
    write(drama, arguments)
  end

  private

  def require!(condition, code)
    ProductionTextRules.require!(condition, code)
  end

  def copy(value)
    JSON.parse(JSON.generate(value))
  end

  def snapshot_digest(series)
    ScreenplayRules.digest(series.slice("sourceFrozen", "sourceCandidates", "sourceCanon"))
  end

  def normalize(raw)
    candidate = copy(raw)
    candidate["scenes"].each do |scene|
      next if scene["beats"]
      scene["beats"] = [{ "action" => scene.fetch("action") }] + scene.fetch("dialogue")
      scene.delete("action")
      scene.delete("dialogue")
    end
    candidate
  end

  def write(drama, arguments)
    series, input = drama["production"], arguments["input"]
    order = input.fetch("episodeOrder")
    prefix = (1...order).find { |previous| !series["episodes"][previous.to_s]["metadataReady"] }
    if prefix
      @text.annotate_source(drama, prefix, adopt: true)
      return { "pending" => true, "annotatedEpisode" => prefix }
    end
    target = Array(input["previousAttempts"]).last&.fetch("target", nil)
    if !target || target == arguments["taskID"]
      output = @text.execute(drama, input)
      series["episodes"][order.to_s].merge!("sourceInherited" => false, "metadataReady" => true)
      series["sourceDerivative"]["changedEpisodes"] |= [order]
      return output
    end
    node = series["episodes"][order.to_s]
    @text.annotate_source(drama, order, adopt: false) unless node["metadataReady"]
    @text.invalidate_episode_review(drama, order)
    { "episodeOrder" => order, "candidateBasis" => ScreenplayRules.digest(node["candidate"]), "reusedCandidate" => true }
  end

  def frozen_result(drama, input)
    coverage = input["episodeOrder"] ? [input["episodeOrder"]] : input["range"] ? (input["range"]["first"]..input["range"]["last"]).to_a : (input["stage"] == "review_series_plan" ? [0] : (1..drama["episodeCount"]).to_a)
    output = { "title" => drama["title"], "dramaID" => drama["id"], "sourceAcceptance" => drama["production"]["sourceFrozen"].slice("workflowID", "basis", "sha256") }
    output["review"] = { "verdict" => "pass", "mustFix" => 0, "coverage" => coverage } if input["stage"].start_with?("review_")
    output
  end
end

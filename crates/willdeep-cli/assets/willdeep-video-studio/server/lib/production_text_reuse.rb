# frozen_string_literal: true

# Dependency invalidation requires a fresh review, not an automatic rewrite of
# an unchanged, previously accepted candidate. Runtime still schedules both gates.
module ProductionTextReuse
  private

  def reusable_episode(drama, input, task_id, issues)
    return unless task_id && issues.empty?
    history = Array(input["previousAttempts"])
    cause = history.last
    return unless cause.is_a?(Hash) && cause["trigger"].is_a?(Hash) &&
      cause["target"].is_a?(String) && cause["target"] != task_id
    order = input.fetch("episodeOrder")
    node = state(drama)["episodes"].fetch(order.to_s)
    candidate = node["candidate"]
    return unless candidate.is_a?(Hash)
    basis = ScreenplayRules.digest(candidate)
    return unless node["adoptedBasis"] == basis &&
      drama["episodes"][order - 1]["script"] == ScreenplayRules.render(candidate) &&
      history.any? { |entry| entry.dig("result", "candidateBasis") == basis }
    record = { "candidateBasis" => basis, "attempt" => input["attempt"], "invalidatedBy" => cause["target"] }
    begin
      ProductionTextRules.episode!(candidate, order, spec(drama), state(drama)["plan"], state(drama)["outlines"][order.to_s])
    rescue ProductionTextRules::Invalid => error
      record["validationError"] = error.message
    end
    record
  end

  def reuse_episode(drama, input, record)
    order = input.fetch("episodeOrder")
    node = state(drama)["episodes"].fetch(order.to_s)
    invalidate_episode_review(drama, order)
    invalidate_acceptance(drama)
    (node["reuseHistory"] ||= []) << record
    result(drama, input, "candidateBasis" => record["candidateBasis"], "episodeOrder" => order,
      "reusedCandidate" => true, "requiresIndependentReview" => true)
  end
end

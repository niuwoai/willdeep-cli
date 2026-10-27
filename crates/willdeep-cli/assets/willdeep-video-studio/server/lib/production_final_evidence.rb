# frozen_string_literal: true

# Earlier reviews can nominate a defect, but a new audit of the current scripts
# can turn that allegation into a screenplay rework instruction.
module ProductionFinalEvidence
  private

  def final_script_review(drama, input, overview_source, coverage)
    series = state(drama)
    basis = ScreenplayRules.digest([scripts_basis(drama), overview_source, input["attempt"],
      script_review_contract(series, [coverage])])
    record = series["finalReviewEvidence"]
    unless record && record["basis"] == basis
      (series["finalReviewHistory"] ||= []) << JSON.parse(JSON.generate(record)) if record
      record = { "basis" => basis, "attempt" => input["attempt"], "rechecks" => [] }
      series["finalReviewEvidence"] = record
    end
    unless record["overview"]
      record["overview"] = review(drama, input, overview_source,
        { "auditScope" => "Overview and prior review summaries only. Content allegations require a later audit of actual current scripts; do not treat a summary omission as proof of absence in the screenplay." },
        coverage: coverage, role: "final_overview_editor", prior_issues: [], record: false)
      return result(drama, input, "pending" => true, "finalOverviewRead" => true)
    end
    allegations = final_allegations(drama, input, record)
    checked = record["rechecks"].flat_map { |entry| entry["coverage"] }
    pending = allegations.reject { |issue| checked.include?(issue["episodeOrder"]) }
    unless pending.empty?
      order = pending.map { |issue| issue["episodeOrder"] }.min
      actual_orders = ((order - 1)..(order + 1)).select { |value| coverage.include?(value) }
      source = actual_orders.map { |value| "第#{value}集\n#{drama['episodes'][value - 1].fetch('script')}" }.join("\n\n")
      ProductionTextRules.require!(source.bytesize <= ProductionText::SOURCE_LIMIT, "production_final_evidence_source_too_large")
      verdict = review(drama, input, source, { "plan" => series["plan"],
        "contracts" => actual_orders.map { |value| series["outlines"].fetch(value.to_s) },
        "priorState" => grounded_story_state(series, actual_orders.first),
        "auditScope" => "These are actual current scripts. Verify every allegation against these scenes and grounded prior state; earlier reviews and summaries are claims, not facts. Missing details in an earlier review do not establish missing details or contradictory object identities in these scripts." },
        coverage: actual_orders, role: "final_editor", prior_issues: allegations,
        finding_validator: ->(value) { validate_final_sources!(drama, actual_orders, value) })
      record["rechecks"] << { "coverage" => actual_orders, "sourceDigest" => ScreenplayRules.digest(source), "verdict" => verdict }
      return result(drama, input, "pending" => true, "verifiedFinalEpisodes" => (checked + actual_orders).uniq.length)
    end
    verdicts = [record["overview"], *record["rechecks"].map { |entry| entry["verdict"] }]
    output = reviewed(drama, input, verdicts)
    if output.dig("review", "mustFix").zero?
      series["scriptsAccepted"] = { "basis" => scripts_basis(drama), "evidenceBasis" => basis,
        "verdict" => { "status" => output["review"]["verdict"], "coverage" => coverage,
          "summary" => output["review"]["summaries"].join("\n"), "issues" => [] } }
    end
    output
  end

  def final_allegations(drama, input, record)
    proposed = record["overview"].fetch("issues").select { |issue| ScreenplayRules.mandatory?(issue) }.map do |issue|
      issue.merge("id" => ScreenplayRules.digest(["final-overview", record["basis"], issue])[0, 24],
        "scope" => scope(input), "reviewer" => "final_editor", "state" => "proposed")
    end
    (proposed + open_issues(drama, scope(input))).uniq { |issue| issue["id"] }
  end

  def validate_final_sources!(drama, coverage, verdict)
    verdict.fetch("issues").each do |issue|
      order = issue["episodeOrder"]
      script = coverage.include?(order) && drama["episodes"][order - 1]["script"]
      ProductionTextRules.require!(script && script.include?(issue.fetch("excerpt")),
        "production_final_issue_source_mismatch: episodeOrder=#{order}; sourceRef must quote actual script text from that episode, not a neighboring episode or synthetic heading")
    end
    verdict.fetch("resolved", []).each do |entry|
      ProductionTextRules.require!(coverage.any? { |order| drama["episodes"][order - 1]["script"].include?(entry.fetch("evidence")) },
        "production_final_resolution_source_mismatch: choose actual current script evidence, not a synthetic heading")
    end
  end
end

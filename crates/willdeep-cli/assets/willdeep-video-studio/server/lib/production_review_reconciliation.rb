# frozen_string_literal: true

require_relative "production_review_citations"

# Old findings are allegations to reassess, not the template for a fresh audit.
# Every old ID gets an explicit model decision; new independent findings survive.
class ProductionReviewReconciliation
  def initialize(source, previous, coverage)
    @source, @previous, @coverage = source, previous, coverage
    @citations = ProductionReviewCitations.new(source)
  end

  def material(fresh)
    @citations.material.merge("expectedCoverage" => @coverage, "independentAudit" => fresh, "previousIssues" => @previous.map do |issue|
      issue.merge("oldExcerptPresentInCurrentSource" => @source.include?(issue.fetch("excerpt")))
    end)
  end

  def schema
    issue = @citations.schema(@coverage).dig("properties", "issues", "items")
    {
      "type" => "object", "additionalProperties" => false,
      "required" => %w[sourceDigest summary assessments],
      "properties" => {
        "sourceDigest" => { "const" => @citations.digest },
        "summary" => { "type" => "string", "minLength" => 1, "maxLength" => 1_500 },
        "assessments" => {
          "type" => "array", "minItems" => @previous.length, "maxItems" => @previous.length,
          "items" => {
            "type" => "object", "additionalProperties" => false,
            "required" => %w[id decision reason sourceRef],
            "allOf" => [{ "if" => { "properties" => { "decision" => { "const" => "open" } } },
              "then" => { "required" => ["issue"] }, "else" => { "not" => { "required" => ["issue"] } } }],
            "properties" => {
              "id" => { "enum" => @previous.map { |entry| entry.fetch("id") } },
              "decision" => { "enum" => %w[resolved open] },
              "reason" => { "type" => "string", "minLength" => 1, "maxLength" => 1_000 },
              "sourceRef" => { "type" => "string" }, "issue" => issue
            }
          }
        }
      }
    }
  end

  def validate!(value)
    ScreenplayRules.require!(value.is_a?(Hash), "production_reconciliation_object_required")
    ScreenplayRules.text!(value["summary"], "reconciliation summary", 1_500)
    entries = value["assessments"]
    ids = @previous.map { |entry| entry.fetch("id") }.sort
    ScreenplayRules.require!(entries.is_a?(Array) && entries.all? { |entry| entry.is_a?(Hash) && entry["id"].is_a?(String) } &&
      entries.map { |entry| entry["id"] }.sort == ids, "production_reconciliation_exact_previous_ids_required")
    entries.each_with_index do |entry, index|
      ScreenplayRules.require!(%w[resolved open].include?(entry["decision"]), "production_reconciliation_decision_invalid")
      ScreenplayRules.text!(entry["reason"], "reconciliation reason", 1_000)
      ScreenplayRules.require!(entry["decision"] == "open" ? entry["issue"].is_a?(Hash) : !entry.key?("issue"),
        "production_reconciliation_open_issue_required: assessments[#{index}].issue must be a current finding object with rule, episodeOrder, level, severity, sourceRef, reason and suggestion when decision=open; omit issue when decision=resolved")
    end
    proof = { "sourceDigest" => value["sourceDigest"], "issues" => entries.select { |entry| entry["decision"] == "open" }.map { |entry| entry["issue"] },
      "resolved" => entries.map { |entry| entry.slice("id", "sourceRef") } }
    @citations.resolve!(proof)
    verdict = { "status" => proof["issues"].any? { |issue| ScreenplayRules.mandatory?(issue) } ? "block" : "pass",
      "summary" => value["summary"], "coverage" => @coverage, "issues" => proof["issues"], "resolved" => proof["resolved"] }
    ScreenplayRules.review!(verdict, @source, @coverage)
    value["normalized"] = verdict
    value
  end

  def combine(fresh, assessment)
    resolved = assessment.fetch("normalized").fetch("resolved").select do |entry|
      assessment["assessments"].any? { |item| item["id"] == entry["id"] && item["decision"] == "resolved" }
    end
    issues = (fresh.fetch("issues") + assessment.fetch("normalized").fetch("issues")).uniq
    status = issues.any? { |issue| ScreenplayRules.mandatory?(issue) } ? "block" : [fresh["status"], assessment.dig("normalized", "status")].include?("warn") ? "warn" : "pass"
    fresh.merge("status" => status, "summary" => [fresh["summary"][0, 700], assessment["summary"][0, 700]].join("\n"),
      "issues" => issues, "resolved" => resolved, "independentAudit" => fresh, "priorIssueReconciliation" => assessment)
  end
end

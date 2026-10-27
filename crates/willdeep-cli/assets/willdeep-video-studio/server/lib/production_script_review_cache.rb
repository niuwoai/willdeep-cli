# frozen_string_literal: true
require "digest"

module ProductionScriptReviewCache
  CONTRACT_SOURCES = %w[production_text.rb production_text_contracts.rb production_review_citations.rb
    production_review_reconciliation.rb production_knowledge_evidence.rb production_text_review_policy.rb
    production_script_review_cache.rb production_final_evidence.rb production_text_rules.rb screenplay_rules.rb].freeze
  IMPLEMENTATION_DIGEST = Digest::SHA256.hexdigest(CONTRACT_SOURCES.map do |name|
    "#{name}\0#{Digest::SHA256.file(File.join(__dir__, name)).hexdigest}"
  end.join("\0")).freeze

  private

  def script_review_contract(series, part_orders)
    ScreenplayRules.digest([IMPLEMENTATION_DIGEST, ProductionKnowledgeEvidence::SEMANTICS,
      ProductionTextReviewPolicy::INSTRUCTION, @skills.compose(stage: "productionReview", common: []).fetch("system"), part_orders,
      series.fetch("plan"), part_orders.flatten.map { |order| series.fetch("outlines").fetch(order.to_s) }])
  end

  def script_review_record(series, input, source_basis, part_orders)
    key = scope(input)
    contract = script_review_contract(series, part_orders)
    record = series.fetch("scriptReviews")[key]
    return record if record && record["basis"] == source_basis && record["attempt"] == input["attempt"] &&
      record["contractBasis"] == contract
    if record
      (series["scriptReviewHistory"] ||= []) << { "scope" => key, "record" => JSON.parse(JSON.generate(record)) }
    end
    { "basis" => source_basis, "contractBasis" => contract, "attempt" => input["attempt"], "parts" => [] }
  end
end

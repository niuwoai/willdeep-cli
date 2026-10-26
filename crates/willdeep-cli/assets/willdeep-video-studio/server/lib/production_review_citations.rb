# frozen_string_literal: true

require_relative "screenplay_rules"
require_relative "production_text_contracts"

# Model-selected references bind evidence to the exact current source. No fuzzy
# matching or replacement of a model's conflicting literal quote is permitted.
class ProductionReviewCitations
  SLICE_LENGTH = 500

  def initialize(source)
    @digest = ScreenplayRules.digest(source)
    slices = source.lines.flat_map { |line| line.scan(/.{1,#{SLICE_LENGTH}}/m) }.map(&:strip).reject(&:empty?)
    @references = slices.each_with_index.to_h { |text, index| ["#{@digest[0, 16]}:#{index + 1}", text] }
  end

  attr_reader :digest

  def material
    { "sourceDigest" => @digest, "source" => @references.map { |id, text| { "sourceRef" => id, "text" => text } } }
  end

  def schema(coverage)
    contract = ProductionTextContracts.review(coverage)
    contract["properties"]["sourceDigest"] = { "const" => @digest }
    contract["required"] << "sourceDigest"
    { "issues" => "excerpt", "resolved" => "evidence" }.each do |field, literal|
      item = contract.dig("properties", field, "items")
      item["properties"].delete(literal)
      item["required"].delete(literal)
      item["properties"]["sourceRef"] = { "type" => "string", "pattern" => "^#{@digest[0, 16]}:[1-9][0-9]*$" }
      item["required"] << "sourceRef"
    end
    contract
  end

  def resolve!(value)
    ScreenplayRules.require!(value.is_a?(Hash) && value["sourceDigest"] == @digest, "production_review_source_digest_mismatch")
    resolved = JSON.parse(JSON.generate(value))
    { "issues" => "excerpt", "resolved" => "evidence" }.each do |field, literal|
      entries = resolved[field]
      ScreenplayRules.require!(entries.is_a?(Array), "production_review_citations_invalid: #{field}")
      entries.each_with_index do |entry, index|
        ScreenplayRules.require!(entry.is_a?(Hash), "production_review_citation_invalid: #{field}[#{index}]")
        text = @references[entry["sourceRef"]]
        ScreenplayRules.require!(text && !entry.key?(literal), "production_review_citation_invalid: #{field}[#{index}].sourceRef; choose an exact current sourceRef and omit #{literal}")
        entry[literal] = text
      end
    end
    value.replace(resolved)
  end
end

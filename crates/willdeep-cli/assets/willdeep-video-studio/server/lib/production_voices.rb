# frozen_string_literal: true

require_relative "production_text_rules"
require_relative "reference_package"

# Stock presets are operator configuration, never model-invented voice IDs.
# Existing personal voice assets keep their original, independently granted consent.
class ProductionVoices
  def self.configured_presets(env: ENV)
    hosted = env["VIDEO_STUDIO_HOST_STOCK_VOICES"].to_s.strip
    return hosted unless hosted.empty?
    original = env["VIDEO_STUDIO_PRODUCTION_STOCK_VOICES"].to_s.strip
    original.empty? ? "[]" : original
  end

  def initialize(presets:, provider: nil)
    @presets, @provider = presets, provider
  end

  def available?
    !usable_presets.empty?
  rescue JSON::ParserError, ProductionTextRules::Invalid
    false
  end

  def configuration
    values = catalog
    usable = usable_presets
    { "state" => values.empty? ? "missing" : usable.empty? ? "incompatible" : "configured", "count" => usable.length }
  rescue JSON::ParserError, ProductionTextRules::Invalid
    { "state" => "invalid", "count" => 0 }
  end

  def selection_catalog
    usable_presets.map { |preset| preset.slice("id", "name") }
  end

  def assign(drama, episode, selections: {})
    names = episode["shots"].flat_map { |shot| shot["dialogue"].map { |line| line["speaker"] } }.uniq
    presets = usable_presets
    names.each do |name|
      character = drama["characters"].find { |entry| entry["name"] == name }
      existing = drama["assets"].find { |entry| entry["kind"] == "voice" && entry["characterID"] == character["id"] && !entry["archived"] && ReferencePackage.consent_granted?(entry) }
      unless existing
        ProductionTextRules.require!(!presets.empty?, "production_stock_voices_unconfigured")
        ProductionTextRules.require!(drama["assets"].length < DramaService::MAX_ASSETS, "production_asset_limit_reached")
        preset = presets.find { |entry| entry["id"] == selections[name] }
        preset ||= presets.first if presets.length == 1 && !selections.key?(name)
        ProductionTextRules.require!(preset, "production_character_voice_selection_required")
        existing = JSON.parse(JSON.generate(DramaService::ASSET_DEFAULTS.merge(DramaService::ASSET_KIND_DEFAULTS["voice"])))
        existing.merge!("id" => SecureRandom.uuid, "kind" => "voice", "name" => "#{name} · #{preset['name']}", "characterID" => character["id"],
          "providerVoiceID" => preset["voice"], "productionStockPreset" => preset.slice("id", "provider"),
          "consent" => { "status" => "granted", "grantedBy" => "configured-stock-preset", "confirmedAt" => Time.now.utc.iso8601,
            "note" => "Operator-configured provider stock preset; no voice cloning.", "presetDigest" => ScreenplayRules.digest(preset) })
        drama["assets"] << existing
      end
      episode["shots"].each do |shot|
        binding = shot.dig("package", "cast").find { |entry| entry["characterID"] == character["id"] }
        binding["voiceID"] = existing["id"] if binding
      end
    end
  end

  private

  def usable_presets
    values = catalog
    return values unless @provider
    backend = @provider.call
    values.select { |entry| entry["provider"] == backend }
  end

  def catalog
    raw = @presets.call
    values = raw.is_a?(String) ? JSON.parse(raw) : raw
    ProductionTextRules.require!(values.is_a?(Array) && values.length <= 20, "production_stock_voice_catalog_invalid")
    values.each do |entry|
      ProductionTextRules.require!(entry.is_a?(Hash) && entry["kind"] == "stock", "production_stock_voice_catalog_invalid")
      %w[id name provider voice].each { |key| ProductionTextRules.text!(entry[key], key, 160) }
    end
    ProductionTextRules.require!(values.map { |entry| entry["id"] }.uniq.length == values.length, "production_stock_voice_catalog_duplicate")
    values
  end
end

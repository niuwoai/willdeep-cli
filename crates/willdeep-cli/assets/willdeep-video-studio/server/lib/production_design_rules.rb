# frozen_string_literal: true

require_relative "production_text_rules"

# Code owns durations, dialogue and identities. Models may propose only creative fields.
module ProductionDesignRules
  extend self
  FIELDS = { "title" => 160, "summary" => 2_000, "actionStart" => 2_000, "actionEnd" => 2_000,
             "cameraIntent" => 1_000, "soundscape" => 1_000, "music" => 500,
             "startPrompt" => 8_000, "endPrompt" => 8_000 }.freeze
  KEY = /\A[a-z][a-z0-9_-]{0,63}\z/
  BATCH = 6

  def require!(condition, code)
    ProductionTextRules.require!(condition, code)
  end

  def key!(value)
    require!(value.is_a?(String) && KEY.match?(value), "production_asset_key_invalid")
  end

  def lines(candidate)
    candidate.fetch("scenes").flat_map do |scene|
      scene.fetch("beats").each_with_index.map do |beat, index|
        next unless beat["speaker"]
        { "sourceID" => "#{scene['id']}d#{index + 1}", "sceneID" => scene["id"], "speaker" => beat["speaker"], "text" => beat["text"] }
      end.compact
    end
  end

  def slots(candidate, seconds)
    scenes = candidate.fetch("scenes")
    require!(scenes.length * 4 <= seconds, "production_too_many_scenes_for_duration")
    weights = scenes.map { |scene| JSON.generate(scene["beats"]).length }
    remaining = seconds - scenes.length * 4
    allocated = weights.map { |weight| 4 + remaining * weight / weights.sum }
    (seconds - allocated.sum).times { |index| allocated[index % allocated.length] += 1 }
    all_lines = lines(candidate)
    result = scenes.each_with_index.flat_map do |scene, index|
      duration = allocated[index]
      count = (duration / 15.0).ceil
      dialogue = all_lines.select { |line| line["sceneID"] == scene["id"] }
      count.times.map do |part|
        selected = dialogue.each_with_index.select { |_line, position| position * count / [dialogue.length, 1].max == part }.map(&:first)
        { "sceneID" => scene["id"], "duration" => duration / count + (part < duration % count ? 1 : 0),
          "dialogueIDs" => selected.map { |line| line["sourceID"] } }
      end
    end
    result.each_with_index { |slot, index| slot["order"] = index + 1 }
    require!(result.length <= 40 && result.all? { |slot| slot["duration"].between?(4, 15) }, "production_shot_duration_invalid")
    result
  end

  def shots!(value, expected, candidate, names)
    require!(value.is_a?(Hash) && value["shots"].is_a?(Array) && value["shots"].length == expected.length, "production_shot_coverage_invalid")
    dialogue = lines(candidate)
    value["shots"].zip(expected).each do |shot, slot|
      require!(shot.is_a?(Hash) && shot.slice(*slot.keys) == slot, "production_shot_contract_changed")
      FIELDS.each { |key, maximum| ProductionTextRules.text!(shot[key], key, maximum) }
      key!(shot["sceneKey"])
      require!(shot["cast"].is_a?(Array) && shot["cast"].length.between?(1, names.length), "production_shot_cast_invalid")
      cast_names = shot["cast"].map do |entry|
        require!(entry.is_a?(Hash) && names.include?(entry["name"]), "production_shot_character_invalid")
        key!(entry["appearanceKey"])
        entry["name"]
      end
      require!(cast_names.uniq == cast_names && (dialogue.select { |line| slot["dialogueIDs"].include?(line["sourceID"]) }.map { |line| line["speaker"] } - cast_names).empty?, "production_dialogue_speaker_missing")
      require!(shot["propKeys"].is_a?(Array) && shot["propKeys"].length <= 8 && shot["propKeys"].uniq == shot["propKeys"], "production_shot_props_invalid")
      shot["propKeys"].each { |key| key!(key) }
    end
    value
  end

  def definitions(shots)
    shots.flat_map do |shot|
      [{ "key" => shot["sceneKey"], "kind" => "scene" }] +
        shot["cast"].map { |entry| { "key" => entry["appearanceKey"], "kind" => "appearance", "character" => entry["name"] } } +
        shot["propKeys"].map { |key| { "key" => key, "kind" => "prop" } }
    end.uniq
  end

  def assets!(value, expected, names)
    require!(value.is_a?(Hash) && value["assets"].is_a?(Array), "production_asset_definitions_invalid")
    require!(value["assets"].map { |entry| entry.slice("key", "kind", "character") } == expected, "production_asset_coverage_invalid")
    value["assets"].each do |entry|
      %w[name prompt notes].each { |key| ProductionTextRules.text!(entry[key], key, key == "prompt" ? 8_000 : key == "notes" ? 4_000 : 120) }
    end
    require!(value["characters"].is_a?(Array) && value["characters"].map { |entry| entry["name"] } == names, "production_visual_character_coverage_invalid")
    value["characters"].each { |entry| ProductionTextRules.text!(entry["visualPrompt"], "visualPrompt", 8_000) }
    ProductionTextRules.text!(value["musicPrompt"], "musicPrompt", 2_000)
    value
  end
end

# frozen_string_literal: true

require_relative "production_design_rules"
require_relative "drama_service"

class ProductionDesign
  STAGES = %w[storyboard review_storyboard design_assets].freeze

  def initialize(text:, voices: nil)
    @text, @voices = text, voices
  end

  def execute(drama, input)
    case input["stage"]
    when "storyboard" then storyboard(drama, input)
    when "review_storyboard" then review(drama, input)
    when "design_assets" then assets(drama, input)
    end
  end

  private

  def require!(condition, code)
    ProductionDesignRules.require!(condition, code)
  end

  def context(drama, input)
    order = input.fetch("episodeOrder")
    series = drama.fetch("production")
    node = series.fetch("episodes").fetch(order.to_s, nil)
    frozen = series["sourceFrozen"] && !series["sourceDerivative"]
    require!(frozen || node && node["adoptedBasis"] == ScreenplayRules.digest(node["candidate"]), "production_script_not_accepted")
    require!(frozen || series["scriptsAccepted"], "production_series_scripts_not_accepted")
    scripts_basis = ScreenplayRules.digest([series["plan"], series["outlines"], drama["episodes"].map { |entry| entry.slice("order", "script") }])
    if series["designScriptsBasis"]
      require!(series["designScriptsBasis"] == scripts_basis, "production_accepted_scripts_changed")
    else
      unless frozen
        accepted = ScreenplayRules.digest([series["plan"], series["outlines"], drama["episodes"].map { |entry| entry.slice("order", "rev", "script", "draftRev", "draft") }])
        require!(series["scriptsAccepted"]["basis"] == accepted, "production_accepted_scripts_changed")
      end
      series["designScriptsBasis"] = scripts_basis
    end
    candidate = node ? node.fetch("candidate") : series.fetch("sourceCandidates").fetch(order.to_s)
    board = (series["storyboards"] ||= {})[order.to_s] ||= { "history" => [] }
    [series, drama["episodes"][order - 1], candidate, board]
  end

  def result(drama, input, extra = {})
    { "dramaID" => drama["id"], "episodeOrder" => input["episodeOrder"] }.merge(extra)
  end

  def catalog_index(series)
    series.fetch("assetCatalog", {}).transform_values do |entry|
      entry.slice("kind", "character").merge("name" => entry["name"].to_s[0, 40])
    end
  end

  def shot_briefs(shots)
    shots.map do |shot|
      shot.slice("order", "sceneID", "duration", "sceneKey", "cast", "propKeys").merge(
        "summary" => shot["summary"].to_s[0, 200], "actionStart" => shot["actionStart"].to_s[0, 200],
        "actionEnd" => shot["actionEnd"].to_s[0, 200], "cameraIntent" => shot["cameraIntent"].to_s[0, 150])
    end
  end

  def storyboard(drama, input)
    series, _episode, candidate, board = context(drama, input)
    if board["attempt"] != input["attempt"]
      board["history"] << JSON.parse(JSON.generate(board.reject { |key, _| key == "history" })) if board["candidate"]
      board.merge!("attempt" => input["attempt"], "candidate" => [], "acceptedBasis" => nil)
      board.delete("design")
      board.delete("musicPrompt")
      board.delete("reviewParts")
    end
    slots = ProductionDesignRules.slots(candidate, series["spec"]["episode_seconds"])
    expected = slots.drop(board["candidate"].length).first(ProductionDesignRules::BATCH)
    unless expected.empty?
      instruction = '把已定稿剧本拍成连续分镜。输出 {shots:[{order,sceneID,duration,dialogueIDs,title,summary,actionStart,actionEnd,cameraIntent,soundscape,music,startPrompt,endPrompt,sceneKey,cast:[{name,appearanceKey}],propKeys:[]}]}。每项严格复制expectedSlots的order/sceneID/duration/dialogueIDs，不增删、换序或改写任何台词；动作只来自对应场次。所有文字字段完整非空；无需音乐写music="N/A"。首尾帧提示词描述静止构图、人物年龄外形衣着、动作起止、空间光照，不含字幕。场景、服装、道具key为小写英文语义标识，优先复用catalog相同设定；不同角色服装不能共用key。重要道具不得遗漏。'
      wanted = expected.flat_map { |slot| slot["dialogueIDs"] }
      output = @text.checked("分镜导演", instruction, { "script" => candidate, "style" => drama["visualStyle"],
        "expectedSlots" => expected, "dialogueSource" => ProductionDesignRules.lines(candidate).select { |line| wanted.include?(line["sourceID"]) }, "previousShots" => shot_briefs(board["candidate"].last(2)),
        "catalog" => catalog_index(series), "revisionNotes" => input["previousAttempts"] }, writing: true) do |value|
        ProductionDesignRules.shots!(value, expected, candidate, drama["characters"].map { |entry| entry["name"] })
      end
      board["candidate"].concat(output["shots"])
    end
    result(drama, input, "pending" => board["candidate"].length < slots.length, "plannedShots" => board["candidate"].length, "totalShots" => slots.length)
  end

  def review(drama, input)
    _series, _episode, candidate, board = context(drama, input)
    shots = board.fetch("candidate")
    require!(shots.length == ProductionDesignRules.slots(candidate, drama["production"]["spec"]["episode_seconds"]).length, "production_storyboard_incomplete")
    parts = shots.each_slice(ProductionDesignRules::BATCH).to_a
    reviews = board["reviewParts"] ||= []
    index = reviews.length
    require!(index < parts.length, "production_storyboard_review_receipt_missing")
    first = parts[index].first["order"]
    last = parts[index].last["order"]
    neighbors = shots.select { |shot| shot["order"] == first - 1 || shot["order"] == last + 1 }
    output = @text.inspect_material(drama, input, source: JSON.generate(parts[index]), material: { "script" => candidate,
      "expectedShotOrders" => parts[index].map { |shot| shot["order"] }, "neighborShots" => shot_briefs(neighbors),
      "checks" => "完整审读本批每镜，逐场覆盖、对白顺序、动作因果、与前后邻镜连续性、首尾帧与身份一致、对白可在时长内说完。" },
      target: "storyboard:#{input['episodeOrder']}", coverage: [input["episodeOrder"]], role: "director_part_#{index + 1}")
    reviews << output["review"]
    return result(drama, input, "pending" => true, "reviewedParts" => reviews.length, "totalParts" => parts.length) if reviews.length < parts.length
    output["review"]["verdict"] = "warn" if output["review"]["mustFix"].zero? && reviews.any? { |part| part["verdict"] == "warn" }
    board["review"] = output["review"]
    board["acceptedBasis"] = ScreenplayRules.digest(shots) if output["review"]["mustFix"].zero?
    output
  end

  def assets(drama, input)
    series, episode, candidate, board = context(drama, input)
    shots = board.fetch("candidate")
    require!(board["acceptedBasis"] == ScreenplayRules.digest(shots), "production_storyboard_not_accepted")
    catalog = series["assetCatalog"] ||= {}
    definitions = ProductionDesignRules.definitions(shots)
    require!(definitions.group_by { |entry| entry["key"] }.values.all? { |group| group.length == 1 }, "production_asset_key_collision")
    definitions.each { |entry| require!(!catalog[entry["key"]] || catalog[entry["key"]].slice("kind", "character") == entry.slice("kind", "character"), "production_asset_identity_changed") }
    missing = definitions.reject { |entry| catalog.key?(entry["key"]) }
    names = shots.flat_map { |shot| shot["cast"].map { |entry| entry["name"] } }.uniq
    voice_catalog = @voices ? @voices.selection_catalog : []
    voice_selections = series["voiceSelections"] ||= {}
    instruction = '把分镜中缺少的资产定义成可跨集复用的视觉规范。只输出 {assets:[{key,kind,character(仅appearance),name,prompt,notes}],characters:[{name,visualPrompt}],musicPrompt}。assets严格按expectedAssets逐项复制key/kind/character，不能添加或改变既有catalog；prompt含完整可出图的静态外观和光照，notes写连续性约束。characters严格按expectedCharacters顺序补全视觉身份，不改变已有身份。musicPrompt为本集纯器乐配乐的情绪、乐器和节奏，不含人声或歌词，不模仿在世艺人。'
    instruction += '若voiceCatalog非空，每个characters还必须含voicePresetID，只能从目录选取；结合角色年龄、性别、音色选择，不能按角色顺序轮换。lockedVoiceSelections中的已有选择必须严格复用。' unless voice_catalog.empty?
    design = board["design"] ||= { "assets" => [] }
    expected = missing.reject { |entry| design["assets"].any? { |asset| asset["key"] == entry["key"] } }.first(8)
    generated = @text.checked("美术与音乐设计", instruction, { "shots" => shot_briefs(shots), "expectedAssets" => expected,
      "expectedCharacters" => names, "characters" => drama["characters"].select { |entry| names.include?(entry["name"]) }.map { |entry| entry.slice("name", "visualPrompt") },
      "catalog" => catalog_index(series), "style" => drama["visualStyle"], "voiceCatalog" => voice_catalog,
      "characterRoles" => series.fetch("plan").fetch("characters").select { |entry| names.include?(entry["name"]) },
      "lockedVoiceSelections" => voice_selections }, writing: true) do |value|
      ProductionDesignRules.assets!(value, expected, names)
      validate_voices!(value["characters"], voice_catalog, voice_selections)
    end
    generated["characters"].each { |entry| voice_selections[entry["name"]] = entry["voicePresetID"] } unless voice_catalog.empty?
    design["assets"].concat(generated["assets"])
    return result(drama, input, "pending" => true, "designedAssets" => design["assets"].length, "totalAssets" => missing.length) if design["assets"].length < missing.length
    generated["assets"] = design["assets"]
    require!(drama.fetch("assets", []).length + missing.length <= DramaService::MAX_ASSETS, "production_asset_limit_reached")
    generated["assets"].each do |definition|
      character = drama["characters"].find { |entry| entry["name"] == definition["character"] }
      asset = JSON.parse(JSON.generate(DramaService::ASSET_DEFAULTS.merge(DramaService::ASSET_KIND_DEFAULTS.fetch(definition["kind"]))))
      asset.merge!(definition.slice("kind", "name", "prompt", "notes")).merge!("id" => SecureRandom.uuid, "productionKey" => definition["key"])
      asset["characterID"] = character["id"] if character
      drama["assets"] << asset
      catalog[definition["key"]] = definition.slice("kind", "character", "name", "prompt", "notes").merge("id" => asset["id"])
    end
    generated["characters"].each do |definition|
      character = drama["characters"].find { |entry| entry["name"] == definition["name"] }
      character["visualPrompt"] = definition["visualPrompt"] if character["visualPrompt"].to_s.empty?
    end
    adopt_shots(drama, episode, shots, candidate, catalog, board)
    @voices.assign(drama, episode, selections: voice_selections) if @voices
    board["musicPrompt"] = generated["musicPrompt"]
    result(drama, input, "assetIDs" => definitions.map { |entry| catalog[entry["key"]]["id"] }, "shots" => episode["shots"].length)
  end

  def validate_voices!(characters, catalog, locked)
    return if catalog.empty?
    characters.each do |entry|
      require!(catalog.any? { |preset| preset["id"] == entry["voicePresetID"] }, "production_voice_preset_not_in_catalog")
      require!(!locked.key?(entry["name"]) || locked[entry["name"]] == entry["voicePresetID"], "production_character_voice_selection_changed")
    end
  end

  def adopt_shots(drama, episode, shots, candidate, catalog, board)
    board["adoptedHistory"] ||= []
    board["adoptedHistory"] << episode["shots"] unless episode["shots"].empty?
    require!(episode["shots"].all? { |shot| shot["productionWorkflowID"] == drama["production"]["workflowID"] }, "production_existing_manual_shots_preserved")
    lines = ProductionDesignRules.lines(candidate)
    episode["shots"] = shots.map do |proposed|
      shot = JSON.parse(JSON.generate(DramaService::SHOT_DEFAULTS)).merge(proposed.slice("order", "duration", *ProductionDesignRules::FIELDS.keys))
      shot["id"] = SecureRandom.uuid
      shot["productionWorkflowID"] = drama["production"]["workflowID"]
      shot["productionBoardBasis"] = board["acceptedBasis"]
      shot["dialogue"] = proposed["dialogueIDs"].map { |id| lines.find { |line| line["sourceID"] == id }.slice("speaker", "text", "sourceID").merge("id" => SecureRandom.uuid) }
      shot["package"] = { "sceneID" => catalog[proposed["sceneKey"]]["id"], "propIDs" => proposed["propKeys"].map { |key| catalog[key]["id"] },
        "generationMode" => "auto", "audioRetention" => "fully_copy", "excluded" => [], "videoRefs" => [],
        "cast" => proposed["cast"].map do |entry|
          { "characterID" => drama["characters"].find { |character| character["name"] == entry["name"] }["id"],
            "appearanceID" => catalog[entry["appearanceKey"]]["id"], "role" => "supporting", "screenPosition" => "", "appearanceChange" => "" }
        end }
      shot
    end
    episode["rev"] = episode["rev"].to_i + 1
  end
end

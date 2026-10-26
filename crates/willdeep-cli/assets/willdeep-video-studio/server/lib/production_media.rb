# frozen_string_literal: true

require_relative "production_backends"
require_relative "image_generation"
require_relative "voice_generation"
require_relative "image_qa"

# Adapters own their media writes. No stale DramaStore snapshot is written over a
# backend's newly saved candidates, audio or reviews.
class ProductionMedia
  STAGES = %w[generate_assets review_assets generate_frames review_frames generate_dialogue review_dialogue generate_music review_music generate_video review_video compose_episode review_film review_series deliver].freeze
  CREATIVE_KEYS = %w[id order title summary script visualPrompt name description kind prompt notes characterID
                     providerVoiceID consent package dialogue speaker text sourceID actionStart actionEnd cameraIntent
                     duration soundscape music startPrompt endPrompt draft draftRev rev identityVersion aspectRatio
                     aspectSpec episodeDurationSeconds visualStyle shots cast appearanceID voiceID role screenPosition
                     appearanceChange sceneID sceneVariantID propIDs excluded generationMode videoRefs audioRetention
                     jobID language dialect presets instruction speed productionStockPreset productionWorkflowID productionBoardBasis].freeze

  def initialize(store:, dramas:, host:, image_backend:, speech_backend:, image_qa:, remote: nil, audio: nil, video_audit: nil, composition: nil, film: nil, series: nil, delivery: nil)
    @store, @dramas, @host, @image_backend, @speech_backend, @qa, @remote, @audio, @video_audit = store, dramas, host, image_backend, speech_backend, image_qa, remote, audio, video_audit
    @composition = composition
    @film = film
    @series = series
    @delivery = delivery
  end

  def handles?(stage)
    STAGES.include?(stage)
  end

  def reconcile(arguments, receipts)
    require!(@remote, "production_remote_adapter_unavailable")
    drama = @store.find_by_request_id("production:#{arguments['workflowID']}")
    require!(drama, "production_drama_missing")
    meter = ProductionMeter.new(host: @host, task_id: arguments["taskID"], grant: arguments["grant"],
      input_digest: arguments["inputDigest"], attempt: arguments["input"]["attempt"])
    @remote.reconcile(drama: drama, arguments: arguments, receipts: receipts, meter: meter)
  end

  def rebind_source_metadata(before, after)
    old_basis, new_basis = creative_basis(before), creative_basis(after)
    after["production"].fetch("mediaSteps", {}).each_value do |record|
      next unless record["creativeBasis"] == old_basis
      record["sourceMigrationBasis"] = old_basis
      record["creativeBasis"] = new_basis
    end
  end

  def pipeline_capabilities
    wired = [@remote, @audio, @video_audit, @composition, @film, @series, @delivery].all?
    { "ready" => wired, "audioReview" => !!(@audio && @audio.available?),
      "filmReview" => !!(@film && @series && @video_audit), "delivery" => !@delivery.nil? }
  end

  def step(drama, arguments, basis:, authorize:)
    input = arguments.fetch("input")
    key = "#{arguments['taskID']}:#{input['attempt']}"
    creative = creative_basis(drama)
    receipt = drama.dig("production", "mediaSteps", key)
    if receipt
      require!(receipt["inputDigest"] == arguments["inputDigest"] && receipt["creativeBasis"] == creative, "production_revision_conflict")
      return receipt["result"].merge("deduplicated" => true) if receipt["result"]
    else
      require!(drama["production"]["lastBasis"] == basis.call(drama), "production_revision_conflict")
      update(drama["id"], creative) do |current|
        (current["production"]["mediaSteps"] ||= {})[key] = { "inputDigest" => arguments["inputDigest"], "creativeBasis" => creative, "items" => {},
          "stage" => input["stage"], "episodeOrder" => input["episodeOrder"], "taskID" => arguments["taskID"], "attempt" => input["attempt"] }
      end
    end
    drama = @store.find(drama["id"])
    record = drama["production"]["mediaSteps"][key]
    episode = drama["episodes"].find { |entry| entry["order"] == input["episodeOrder"] }
    require!(%w[review_series deliver].include?(input["stage"]) || episode && !episode["shots"].empty?, "production_storyboard_not_adopted")
    meter = ProductionMeter.new(host: @host, task_id: arguments["taskID"], grant: arguments["grant"], input_digest: arguments["inputDigest"], attempt: input["attempt"])
    items = case input["stage"]
            when "generate_dialogue", "review_dialogue" then dialogue_items(episode)
            when "generate_music", "review_music", "compose_episode", "review_film" then [{ "episodeOrder" => episode["order"] }]
            when "review_series", "deliver" then [{ "range" => input["range"] }]
            when "generate_video", "review_video" then episode["shots"].map { |shot| { "episodeID" => episode["id"], "shotID" => shot["id"] } }
            else image_items(drama, episode, input["stage"].include?("assets"))
            end
    item = items.find { |entry| !record["items"].key?(item_key(entry)) }
    if item
      authorize.call
      output = if input["stage"] == "generate_dialogue"
                 speech(drama, item, meter, key)
               elsif %w[review_dialogue review_music].include?(input["stage"])
                 require!(@audio, "production_audio_review_unavailable")
                 @audio.review(drama: drama, item: item, stage: input["stage"], meter: meter,
                   update: ->(&block) { update(drama["id"], creative, &block) })
               elsif input["stage"] == "review_video"
                 require!(@video_audit, "production_video_review_unavailable")
                 @video_audit.review(drama: drama, item: item, meter: meter,
                   update: ->(&block) { update(drama["id"], creative, &block) })
               elsif input["stage"] == "compose_episode"
                 require!(@composition, "production_composition_unavailable")
                 @composition.step(drama: drama, item: item, arguments: arguments,
                   update: ->(&block) { update(drama["id"], creative, &block) })
               elsif input["stage"] == "review_film"
                 require!(@film, "production_film_review_unavailable")
                 @film.review(drama: drama, item: item, arguments: arguments, meter: meter,
                   update: ->(&block) { update(drama["id"], creative, &block) })
               elsif input["stage"] == "review_series"
                 require!(@series, "production_series_review_unavailable")
                 @series.step(drama: drama, arguments: arguments, meter: meter,
                   update: ->(&block) { update(drama["id"], creative, &block) })
               elsif input["stage"] == "deliver"
                 require!(@delivery, "production_delivery_unavailable")
                 @delivery.step(drama: drama, arguments: arguments, permission: authorize.call,
                   update: ->(&block) { update(drama["id"], creative, &block) })
               elsif %w[generate_music generate_video].include?(input["stage"])
                 require!(@remote, "production_remote_adapter_unavailable")
                 reusable = input["stage"] == "generate_video" && @video_audit && @video_audit.reusable(drama, item)
                 reusable || @remote.step(drama: drama, item: item, arguments: arguments, meter: meter,
                   update: ->(&block) { update(drama["id"], creative, &block) }, authorize: authorize)
               else
                 image(drama, item, meter, key, review_only: input["stage"].start_with?("review_"))
               end
      authorize.call
      unless output["pending"]
        update(drama["id"], creative) { |current| current["production"]["mediaSteps"][key]["items"][item_key(item)] = output }
      end
    end
    latest = @store.find(drama["id"])
    completed = latest["production"]["mediaSteps"][key]["items"]
    rejected = input["stage"].start_with?("generate_") && completed.values.any? { |value| value["accepted"] == false }
    pending = completed.length < items.length && !rejected
    result = { "ok" => true, "workflowID" => arguments["workflowID"], "inputDigest" => arguments["inputDigest"],
      "dramaID" => drama["id"], "episodeOrder" => input["episodeOrder"], "state" => pending ? "waiting_external" : "completed",
      "pending" => pending, "completedItems" => completed.length, "totalItems" => items.length }
    result["artifact"] = completed.values.first["artifact"] if !pending && input["stage"] == "compose_episode"
    result["delivery"] = completed.values.first["delivery"] if !pending && input["stage"] == "deliver"
    if output && output["pending"]
      result["itemProgress"] = { "completed" => output["heardParts"] || output["viewedParts"] || output["exportedEpisodes"], "total" => output["totalParts"] || output["totalEpisodes"], "percent" => output["progress"] }.reject { |_key, value| value.nil? }
    end
    if !pending && input["stage"].start_with?("review_")
      failed = completed.select { |_id, value| !value["accepted"] }
      result["review"] = { "inputDigest" => arguments["inputDigest"], "verdict" => failed.empty? ? "pass" : "block", "mustFix" => failed.length,
        "coverage" => input["episodeOrder"] ? [input["episodeOrder"]] : input["range"] ? (input["range"]["first"]..input["range"]["last"]).to_a : (1..drama["episodeCount"]).to_a,
        "issues" => failed.map { |id, value| { "subject" => id, "details" => value["issues"] } } }
      result["reworkTarget"] = failed.values.map { |value| value["reworkTarget"] }.compact.first || input.fetch("repairTargets", []).first.to_h["id"] unless failed.empty?
    end
    update(drama["id"], creative) do |current|
      current["production"]["mediaSteps"][key]["result"] = result unless pending
      current["production"]["mediaSteps"][key]["progress"] = result.slice("completedItems", "totalItems", "state", "itemProgress")
      current["production"]["lastMediaStep"] = key
      current["production"]["lastBasis"] = basis.call(current)
    end
    result
  end

  private

  def require!(condition, code)
    ProductionTextRules.require!(condition, code)
  end

  def creative_basis(drama)
    project = lambda do |value|
      case value
      when Array then value.map { |entry| project.call(entry) }
      when Hash then value.select { |key, _| CREATIVE_KEYS.include?(key) }.transform_values { |entry| project.call(entry) }
      else value
      end
    end
    ScreenplayRules.digest([project.call(drama), project.call(drama["characters"]), project.call(drama["assets"]), project.call(drama["episodes"]),
      drama["production"].slice("spec", "plan", "outlines", "episodes", "scriptsAccepted", "storyboards", "assetCatalog")])
  end

  def update(id, expected)
    saved = @store.update(id) do |current|
      require!(creative_basis(current) == expected, "production_revision_conflict")
      objects = [current] + current["characters"] + current["assets"] + current["episodes"] + current["episodes"].flat_map { |episode| episode["shots"] }
      require!(objects.none? { |entry| entry["draft"] && !entry["draft"].empty? }, "production_user_draft_pending")
      yield current
    end
    require!(saved, "production_drama_disappeared")
  end

  def image_items(drama, episode, assets)
    unless assets
      return episode["shots"].flat_map { |shot| %w[start end].map { |kind| { "target" => kind, "episodeID" => episode["id"], "shotID" => shot["id"] } } }
    end
    casts = episode["shots"].flat_map { |shot| Array(shot.dig("package", "cast")) }
    characters = casts.map { |entry| entry["characterID"] }.uniq.map { |id| { "target" => "character", "characterID" => id } }
    ids = episode["shots"].flat_map { |shot| [shot.dig("package", "sceneID"), *Array(shot.dig("package", "propIDs"))] } + casts.map { |entry| entry["appearanceID"] }
    assets = ids.compact.uniq.map { |id| drama["assets"].find { |entry| entry["id"] == id } }
    require!(assets.all?, "production_reference_asset_missing")
    characters + assets.sort_by { |entry| %w[scene prop appearance].index(entry["kind"]) || 9 }.map { |entry| { "target" => entry["kind"], "assetID" => entry["id"] } }
  end

  def dialogue_items(episode)
    episode["shots"].flat_map do |shot|
      shot["dialogue"].map { |line| { "episodeID" => episode["id"], "shotID" => shot["id"], "lineID" => line["id"] } }
    end
  end

  def item_key(item)
    ScreenplayRules.digest(item)
  end

  def request_id(key, item)
    "production:#{key}:#{item_key(item)[0, 24]}"
  end

  def speech(drama, item, meter, key)
    backend = @speech_backend.call
    require!(backend, "production_tts_unavailable")
    shot = drama["episodes"].find { |entry| entry["id"] == item["episodeID"] }["shots"].find { |entry| entry["id"] == item["shotID"] }
    line = shot["dialogue"].find { |entry| entry["id"] == item["lineID"] }
    character = drama["characters"].find { |entry| entry["name"] == line["speaker"] }
    voice_id = shot.dig("package", "cast").find { |entry| entry["characterID"] == character["id"] }["voiceID"]
    voice = drama["assets"].find { |entry| entry["id"] == voice_id }
    require!(!voice || !voice["productionStockPreset"] || voice["productionStockPreset"]["provider"] == backend.name, "production_voice_provider_changed")
    if @audio && @audio.accepted_line?(drama, line)
      return { "generated" => [{ "lineID" => line["id"], "candidateID" => line["audio"]["selectedCandidateID"] }], "reused" => true }
    end
    corrections = @audio ? @audio.corrections(drama, line) : []
    adjustment = corrections.empty? ? nil : { instruction: "准确逐字朗读已给定原台词，口齿清晰，自然连贯，角色音色保持一致。" }
    if corrections.any? { |issue| issue["rule"] == "dialogue_too_long" }
      total = shot["dialogue"].sum { |entry| source = @audio.line_source(entry); source ? source["durationMs"].to_i : 0 }
      adjustment[:speed] = [[total / (shot["duration"].to_f * 1000) + 0.1, 1.05].max, 1.7].min
    end
    wrapped = ProductionBackends::Speech.new(backend: backend, meter: meter, identity: item_key(item), adjustment: adjustment)
    service = VoiceGeneration.new(drama_service: @dramas, backend_selector: -> { wrapped })
    result = service.generate(item.merge("dramaID" => drama["id"], "lineIDs" => [item["lineID"]], "requestID" => request_id(key, item)))
    require!(result["ok"] && result.fetch("generated", []).length == 1 && result.fetch("failed", []).empty?, "production_dialogue_generation_failed")
    token = request_id(key, item)
    expected = line.dig("audio", "selectedCandidateID")
    update(drama["id"], creative_basis(drama)) do |current|
      current_shot = current["episodes"].find { |entry| entry["id"] == item["episodeID"] }["shots"].find { |entry| entry["id"] == item["shotID"] }
      current_line = current_shot["dialogue"].find { |entry| entry["id"] == item["lineID"] }
      candidate = current_line["audio"]["candidates"].find { |entry| entry["requestID"] == token }
      require!(candidate, "production_dialogue_candidate_missing")
      require!(current_line["audio"]["selectedCandidateID"] == expected || current_line["audio"]["selectedCandidateID"] == candidate["id"], "production_user_audio_selection_changed")
      current_line["audio"]["selectedCandidateID"] = candidate["id"]
    end
    { "generated" => result["generated"], "repairIssues" => corrections }
  end

  def image(drama, item, meter, key, review_only:)
    common = item.merge("dramaID" => drama["id"])
    recommended = @qa.current_recommendation(drama["id"], item["target"], ImageQA.ids_for(item["target"], item))
    if recommended && recommended["eligible"]
      select(drama, common, recommended["candidateID"])
      return { "accepted" => true, "candidateID" => recommended["candidateID"], "reused" => true }
    end
    if review_only
      return { "accepted" => false, "issues" => ["production_no_approved_candidate"] }
    end
    backend = @image_backend.call
    require!(backend, "production_image_unavailable")
    wrapped = ProductionBackends::Image.new(backend: backend, meter: meter, identity: item_key(item))
    service = ImageGeneration.new(drama_service: @dramas, host: @host, backend_selector: -> { wrapped })
    node = image_node(drama, item)
    candidates = %w[start end].include?(item["target"]) ? node["#{item['target']}Candidates"] : node["candidates"]
    previous = candidates.reverse.find { |candidate| candidate["qa"] && !ImageQA.eligible?(candidate["qa"]) }
    corrections = []
    if previous
      material = @qa.material(drama, item["target"], ImageQA.ids_for(item["target"], item))
      require!(material["ok"], "production_image_repair_material_unavailable")
      issues = Array(previous.dig("qa", "issues")).select { |issue| issue["level"] == "must" }
      corrections = ImageRemediation.sentences(issues, material).map { |entry| entry["text"] }.first(ImageGeneration::EXTRA_DIRECTIVES_LIMIT)
    end
    generated = service.generate(common.merge("countPerModel" => 1, "requestID" => request_id(key, item), "extraDirectives" => corrections))
    require!(generated["ok"] && generated.fetch("generated", []).length == 1 && generated.fetch("failed", []).empty?, "production_image_generation_failed")
    id = generated["generated"][0]["candidateID"]
    checked = @qa.run_tool(common.merge("candidateIDs" => [id]))
    require!(checked["ok"] && checked["failed"].empty? && checked["checked"].length == 1, "production_image_review_unavailable")
    verdict = checked["checked"][0]
    accepted = ImageQA.eligible?(verdict)
    select(drama, common, id) if accepted
    { "accepted" => accepted, "candidateID" => id, "issues" => verdict["issues"], "repairDirectives" => corrections,
      "previousCandidateID" => previous && previous["id"] }
  end

  def image_node(drama, arguments)
    case arguments["target"]
    when "character" then drama["characters"].find { |entry| entry["id"] == arguments["characterID"] }
    when "start", "end"
      drama["episodes"].find { |entry| entry["id"] == arguments["episodeID"] }["shots"].find { |entry| entry["id"] == arguments["shotID"] }
    else drama["assets"].find { |entry| entry["id"] == arguments["assetID"] }
    end
  end

  def select(before, arguments, id)
    target = arguments["target"]
    frame = %w[start end].include?(target)
    field = frame ? (target == "start" ? "selectedStartID" : "selectedEndID") : "selectedCandidateID"
    expected = image_node(before, arguments).fetch(field, nil)
    update(before["id"], creative_basis(before)) do |current|
      node = image_node(current, arguments)
      require!(node[field] == expected || node[field] == id, "production_user_selection_changed")
      collection = frame ? node["#{target}Candidates"] : node["candidates"]
      require!(collection.any? { |candidate| candidate["id"] == id }, "production_image_candidate_missing")
      node[field] = id
      if frame
        node[target == "start" ? "selectedStartAt" : "selectedEndAt"] = Time.now.utc.iso8601
        node[target == "start" ? "selectedStartBy" : "selectedEndBy"] = "batch"
      end
    end
  end
end

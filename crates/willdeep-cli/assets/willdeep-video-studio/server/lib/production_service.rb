# frozen_string_literal: true

require_relative "production_host_diagnostic"

require_relative "production_text"
require_relative "production_meter"
require_relative "production_design"
require_relative "production_audio_backend"
require_relative "production_snapshot"
require_relative "production_source"
require "securerandom"

class ProductionService
  AUTHORIZE = "willdeep/production/authorize"
  MEDIA_STAGES = %w[storyboard review_storyboard design_assets generate_assets review_assets generate_frames review_frames
                    generate_dialogue review_dialogue generate_music review_music generate_video review_video remediate_video
                    compose_episode review_film review_series deliver].freeze
  STAGES = (["preflight"] + ProductionText::STAGES + MEDIA_STAGES).freeze
  MAX_TEXT_ATTEMPTS = 8
  MAX_EPISODE_TEXT_ATTEMPTS = 24
  MAX_MEDIA_ATTEMPTS = 4

  def initialize(store:, host:, settings:, skills:, preflight:, source_service: nil, media: nil, voices: nil, delivery: nil)
    @store, @host, @preflight, @source_service, @media = store, host, preflight, source_service, media
    @text = ProductionText.new(host: host, settings: settings, skills: skills)
    @source = ProductionSource.new(text: @text)
    @design = ProductionDesign.new(text: @text, voices: voices)
    @delivery = delivery
  end

  def step(arguments)
    validate!(arguments)
    permission = authorize(arguments, allow_settlement: true)
    input = arguments.fetch("input")
    if permission["settlementOnly"] == true
      ProductionTextRules.require!(@media, "production_reconciliation_unavailable")
      return normalize(arguments, @media.reconcile(arguments, permission.fetch("receipts")))
    end
    if input["stage"] == "preflight"
      capabilities = @preflight.call(input["spec"])
      return { "ok" => false, "error" => { "code" => "production_dependencies_unavailable", "checks" => capabilities } } unless capabilities.is_a?(Hash) && capabilities["ok"] == true
      return normalize(arguments, "capabilities" => capabilities)
    end
    drama = @store.find_by_request_id("production:#{arguments['workflowID']}")
    return create(arguments, permission) unless drama
    if drama["production"]["sourceFrozen"]
      return migrate_source(arguments, permission, drama) unless drama["production"]["sourceCanon"]
      @source.verify!(drama)
    end

    if @media && @media.handles?(input["stage"])
      ensure_drafts_preserved!(drama)
      return @media.step(drama, arguments, basis: method(:basis), authorize: -> { authorize(arguments) })
    end

    state = drama.fetch("production")
    ProductionTextRules.require!(state["spec"] == input["spec"], "production_spec_changed")
    key = receipt_key(arguments)
    cached = state.fetch("receipts")[key]
    if cached && !cached["pending"]
      ProductionTextRules.require!(cached["inputDigest"] == arguments["inputDigest"] && cached["basis"] == basis(drama), "production_revision_conflict")
      return cached.fetch("result").merge("deduplicated" => true)
    end
    if cached
      ProductionTextRules.require!(cached["inputDigest"] == arguments["inputDigest"] && cached["basis"] == basis(drama), "production_revision_conflict")
    end
    before = basis(drama)
    ensure_drafts_preserved!(drama)
    if (ProductionText::STAGES + ProductionDesign::STAGES).include?(input["stage"]) && state["lastBasis"]
      ProductionTextRules.require!(state["lastBasis"] == before, "production_revision_conflict")
    end
    if input["stage"] == "series_plan" && state["creationKey"] == key
      ProductionTextRules.require!(state["creationInputDigest"] == arguments["inputDigest"], "production_revision_conflict")
      return finish_creation(arguments, drama)
    end
    output = if state["sourceFrozen"] && ProductionText::STAGES.include?(input["stage"])
               @source.execute(drama, arguments)
             elsif input["stage"] == "series_plan"
               replace_plan(drama, input)
             elsif ProductionText::STAGES.include?(input["stage"])
               @text.execute(drama, input, task_id: arguments["taskID"])
             elsif ProductionDesign::STAGES.include?(input["stage"])
               @design.execute(drama, input)
             else
               raise ProductionTextRules::Invalid, "production_media_adapter_unavailable"
             end
    authorize(arguments)
    result = normalize(arguments, output)
    state = drama.fetch("production")
    state["lastBasis"] = basis(drama)
    state["receipts"][key] = { "inputDigest" => arguments["inputDigest"], "basis" => basis(drama),
                                "result" => result, "pending" => output["pending"] == true }
    saved = @store.update(drama["id"]) do |current|
      ProductionTextRules.require!(basis(current) == before, "production_revision_conflict")
      current.replace(drama)
    end
    ProductionTextRules.require!(saved, "production_drama_disappeared")
    result
  rescue HostBridge::Unsupported
    failure("production_runtime_unsupported")
  rescue HostBridge::RequestFailed => error
    failure("production_host_request_failed", ProductionHostDiagnostic.describe(error))
  rescue ProductionTextRules::Invalid, ProductionMeter::Error, ProductionAudioBackend::Error, ProductionSnapshot::Error, KeyError, JSON::ParserError => error
    failure("production_step_invalid", error.message)
  end

  def status(workflow_id)
    drama = @store.find_by_request_id("production:#{workflow_id}")
    return failure("production_not_found") unless drama
    series = drama.fetch("production")
    { "ok" => true, "dramaID" => drama["id"], "title" => drama["title"], "spec" => series["spec"],
      "written" => drama["episodes"].count { |episode| !episode["script"].to_s.empty? },
      "issues" => series["issues"] + media_issues(series), "source" => series["sourceFrozen"] && series["sourceFrozen"].slice("workflowID", "basis", "sha256"),
      "media" => series.fetch("mediaSteps", {})[series["lastMediaStep"]]&.slice("stage", "episodeOrder", "progress"),
      "revisions" => series["episodes"].values.sum { |node| node.fetch("history", []).length } +
        series.fetch("storyboards", {}).values.sum { |node| node.fetch("history", []).length } }
  end

  def delivery(workflow, selector: nil)
    drama = @store.find_by_request_id("production:#{workflow['id']}")
    ProductionTextRules.require!(drama && @delivery, "production_delivery_unavailable")
    ensure_drafts_preserved!(drama)
    { "ok" => true }.merge(@delivery.read(drama: drama, workflow: workflow, selector: selector))
  rescue ProductionTextRules::Invalid, IOError, SystemCallError, JSON::ParserError, KeyError => error
    failure("production_delivery_invalid", error.message)
  end

  private

  def media_issues(series)
    steps = series.fetch("mediaSteps", {}).values
    steps.flat_map do |step|
      step.fetch("items", {}).flat_map do |id, item|
        issues = Array(item["issues"])
        issues = [{ "reason" => "production_review_not_accepted", "level" => "must" }] if issues.empty? && item["accepted"] == false
        issues.map do |raw|
          issue = raw.is_a?(Hash) ? raw : { "reason" => raw.to_s, "level" => "must" }
          level = issue["level"] == "advice" ? "advice" : "must"
          fixed = steps.any? do |later|
            replacement = later.dig("items", id)
            later["taskID"] == step["taskID"] && later["attempt"].to_i > step["attempt"].to_i && replacement && replacement["accepted"] == true &&
              (level == "must" || Array(replacement["issues"]).none? { |entry| entry.is_a?(Hash) && entry["rule"] == issue["rule"] })
          end
          { "id" => ScreenplayRules.digest([step["taskID"], step["attempt"], id, issue]), "scope" => "#{step['stage']}:#{step['episodeOrder']}",
            "level" => level, "state" => fixed ? "resolved" : "open", "reason" => (issue["reason"] || issue["detail"] || JSON.generate(issue))[0, 1_200] }
        end
      end
    end.last(200)
  end

  def validate!(arguments)
    ProductionTextRules.require!(arguments.is_a?(Hash), "production_step_object_required")
    %w[workflowID taskID grant].each { |key| ProductionTextRules.require!(arguments[key].is_a?(String) && ProductionMeter::UUID.match?(arguments[key]), "production_#{key}_invalid") }
    ProductionTextRules.require!(arguments["inputDigest"].is_a?(String) && arguments["inputDigest"].match?(/\A[0-9a-f]{64}\z/), "production_input_digest_invalid")
    input = arguments["input"]
    ProductionTextRules.require!(input.is_a?(Hash) && STAGES.include?(input["stage"]), "production_stage_invalid")
    spec = input["spec"]
    ProductionTextRules.require!(spec.is_a?(Hash) && spec["episode_count"].is_a?(Integer) && spec["episode_count"].between?(1, 80) &&
      spec["episode_seconds"].is_a?(Integer) && spec["episode_seconds"].between?(30, 300) && %w[portrait landscape square].include?(spec["aspect"]), "production_spec_invalid")
    attempt_limit = if input["imageRetryPolicy"] == "per_material_v1" && %w[generate_assets review_assets generate_frames review_frames].include?(input["stage"])
                      ProductionMedia::IMAGE_BATCH_LIMIT
                    elsif %w[write_episode review_episode].include?(input["stage"])
                      MAX_EPISODE_TEXT_ATTEMPTS
                    else
                      (ProductionText::STAGES + ProductionDesign::STAGES).include?(input["stage"]) ? MAX_TEXT_ATTEMPTS : MAX_MEDIA_ATTEMPTS
                    end
    ProductionTextRules.require!(input["attempt"].is_a?(Integer) && input["attempt"].between?(1, attempt_limit), "production_attempt_invalid")
    if input["episodeOrder"]
      ProductionTextRules.require!(input["episodeOrder"].is_a?(Integer) && input["episodeOrder"].between?(1, spec["episode_count"]), "production_episode_invalid")
    end
    if input["range"]
      ProductionTextRules.require!(input["range"].is_a?(Hash), "production_range_invalid")
      first, last = input["range"].values_at("first", "last")
      ProductionTextRules.require!(first.is_a?(Integer) && last.is_a?(Integer) && first.between?(1, spec["episode_count"]) &&
        last.between?(first, spec["episode_count"]) && last - first < 8, "production_range_invalid")
    end
  end

  def authorize(arguments, allow_settlement: false)
    ProductionTextRules.require!(@host.supports?(AUTHORIZE), "production_runtime_unsupported")
    result = @host.request(AUTHORIZE, arguments)
    ProductionTextRules.require!(result.is_a?(Hash) && result["allowed"] == true &&
      (allow_settlement || result["settlementOnly"] != true), "production_grant_denied")
    result
  end

  def receipt_key(arguments)
    "#{arguments['taskID']}:#{arguments['input']['attempt']}"
  end

  def basis(drama)
    body = drama.reject { |key, _| key == "updatedAt" || key == "production" }
    series = drama.fetch("production").reject { |key, _| %w[receipts lastBasis].include?(key) }
    ScreenplayRules.digest([body, series])
  end

  def ensure_drafts_preserved!(drama)
    objects = [drama] + drama.fetch("characters") + drama.fetch("episodes") + drama.fetch("assets", []) + drama.fetch("episodes").flat_map { |episode| episode.fetch("shots", []) }
    ProductionTextRules.require!(objects.none? { |object| object["draft"] && !object["draft"].empty? }, "production_user_draft_pending")
  end

  def normalize(arguments, output)
    result = output.merge("ok" => true, "workflowID" => arguments["workflowID"], "inputDigest" => arguments["inputDigest"],
      "state" => output["pending"] ? "waiting_external" : "completed")
    result["review"]["inputDigest"] = arguments["inputDigest"] if result["review"]
    result
  end

  def initial_state(arguments, plan)
    { "workflowID" => arguments["workflowID"], "brief" => arguments["input"]["brief"], "spec" => arguments["input"]["spec"],
      "plan" => plan, "outlines" => {}, "outlineReviews" => {}, "episodes" => {}, "storyStates" => {},
      "scriptReviews" => {}, "issues" => [], "receipts" => {}, "history" => [],
      "creationKey" => receipt_key(arguments), "creationInputDigest" => arguments["inputDigest"] }
  end

  def root_plan(plan, spec)
    plan.slice("title", "genre", "logline", "coreConflict", "arc", "tone", "audience", "visualStyle", "characters")
      .merge("episodeCount" => spec["episode_count"], "episodeDurationSeconds" => spec["episode_seconds"],
             "aspectRatio" => { "portrait" => "9:16", "landscape" => "16:9", "square" => "1:1" }.fetch(spec["aspect"]))
  end

  def create(arguments, permission)
    input = arguments.fetch("input")
    ProductionTextRules.require!(input["stage"] == "series_plan", "production_plan_must_be_first")
    if input["sourceScreenplayID"]
      return import_source(arguments, permission)
    end
    plan = @text.create(input)
    state = initial_state(arguments, plan)
    authorize(arguments)
    drama = @store.create(root_plan(plan, state["spec"]), request_id: "production:#{arguments['workflowID']}", production: state)
    finish_creation(arguments, drama)
  end

  def finish_creation(arguments, drama)
    key = receipt_key(arguments)
    cached = drama["production"]["receipts"][key]
    return cached["result"].merge("deduplicated" => true) if cached
    unless drama["production"]["sourceFrozen"]
      expected = root_plan(drama["production"]["plan"], drama["production"]["spec"])
      fields = expected.keys - ["characters"]
      ProductionTextRules.require!(fields.all? { |field| drama[field] == expected[field] }, "production_creation_changed")
      ProductionTextRules.require!(drama["episodes"].all? { |episode| episode["script"].to_s.empty? }, "production_creation_changed")
    end
    result = normalize(arguments, "dramaID" => drama["id"], "title" => drama["title"], "episodeCount" => drama["episodeCount"])
    @store.update(drama["id"]) do |current|
      current["production"]["lastBasis"] = basis(current)
      current["production"]["receipts"][key] = { "inputDigest" => arguments["inputDigest"], "basis" => basis(current), "result" => result }
    end
    result
  end

  def replace_plan(drama, input)
    plan = @text.create(input)
    series = drama["production"]
    series["history"] << { "kind" => "plan", "content" => series["plan"] }
    series["plan"] = plan
    %w[outlines outlineReviews episodes storyStates scriptReviews].each { |key| series[key] = {} }
    series.delete("scriptsAccepted")
    series.delete("boundaryReviews")
    root_plan(plan, series["spec"]).each { |key, value| drama[key] = value unless key == "characters" }
    drama["characters"] = plan["characters"].map do |character|
      previous = drama["characters"].find { |entry| entry["name"] == character["name"] } || { "id" => SecureRandom.uuid, "candidates" => [], "identityVersion" => 1, "draftRev" => 0, "draft" => nil }
      previous.merge(character.slice("name", "visualPrompt")).merge("description" => character["role"], "rev" => previous["rev"].to_i + 1)
    end
    drama["episodes"].each { |episode| episode["script"] = ""; episode["rev"] = episode["rev"].to_i + 1 }
    { "title" => drama["title"], "dramaID" => drama["id"], "episodeCount" => drama["episodeCount"] }
  end

  def import_source(arguments, permission)
    file, source, proof = read_source(arguments, permission)
    spec = arguments["input"]["spec"]
    ProductionTextRules.require!(source["episodes"].length == spec["episode_count"] && source["episodeDurationSeconds"] == spec["episode_seconds"], "production_source_spec_mismatch")
    plan = source.slice("title", "genre", "logline", "coreConflict", "arc", "tone", "audience", "visualStyle")
      .merge("characters" => source["characters"].map { |character| { "name" => character["name"], "role" => character["description"], "visualPrompt" => character["visualPrompt"] } })
    series = initial_state(arguments, plan)
    series["sourceFrozen"] = { "workflowID" => proof["id"], "basis" => proof["domain"]["export"]["basis"],
      "sha256" => file["sha256"], "scripts" => source["episodes"].map { |episode| episode.slice("order", "title", "summary", "script") } }
    series["sourceCandidates"] = source.fetch("screenplay").fetch("episodes").transform_values { |entry| entry["candidate"] }
    @source.prepare!(series, source)
    authorize(arguments)
    drama = @store.create(root_plan(plan, spec), request_id: "production:#{arguments['workflowID']}", production: series)
    @store.update(drama["id"]) do |current|
      source["episodes"].each_with_index do |episode, index|
        current["episodes"][index].merge!(episode.slice("title", "summary", "script"))
      end
    end
    finish_creation(arguments, @store.find(drama["id"]))
  end

  def read_source(arguments, permission)
    proof = permission["sourceScreenplay"]
    ProductionTextRules.require!(@source_service && proof && proof["id"] == arguments["input"]["sourceScreenplayID"], "production_source_not_verified")
    file = @source_service.read_final({ "id" => proof["id"], "format" => "json" }, proof)
    [file, JSON.parse(file.fetch("text")), proof]
  end

  def migrate_source(arguments, permission, drama)
    ensure_drafts_preserved!(drama)
    before = basis(drama)
    series = drama["production"]
    ProductionTextRules.require!(series["lastBasis"] == before, "production_revision_conflict")
    file, source, proof = read_source(arguments, permission)
    frozen = series["sourceFrozen"]
    scripts = source["episodes"].map { |episode| episode.slice("order", "title", "summary", "script") }
    ProductionTextRules.require!(frozen["sha256"] == file["sha256"] && frozen["basis"] == proof["domain"]["export"]["basis"] && frozen["scripts"] == scripts &&
      drama["episodes"].map { |episode| episode.slice("order", "title", "summary", "script") } == scripts, "production_source_snapshot_changed")
    previous = JSON.parse(JSON.generate(drama))
    @source.prepare!(series, source)
    @media.rebind_source_metadata(previous, drama) if @media
    after = basis(drama)
    cached = series["receipts"][receipt_key(arguments)]
    cached["basis"] = after if cached && cached["basis"] == before && cached["inputDigest"] == arguments["inputDigest"]
    series["lastBasis"] = after
    authorize(arguments)
    @store.update(drama["id"]) do |current|
      ProductionTextRules.require!(basis(current) == before, "production_revision_conflict")
      current.replace(drama)
    end
    normalize(arguments, "pending" => true, "sourceMetadataMigrated" => true)
  end

  def failure(code, message = nil)
    { "ok" => false, "error" => { "code" => code, "message" => message } }
  end
end

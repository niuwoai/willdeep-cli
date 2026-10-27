# frozen_string_literal: true
require_relative "host_bridge"

class ProductionControl
  METHOD = "willdeep/production/control"
  UNITS = %w[model_calls tokens images tts_characters music_calls music_seconds video_calls video_seconds].freeze
  LIMITS = { type: "object", additionalProperties: false, properties: UNITS.to_h { |unit|
    [unit, { type: %w[model_calls tokens].include?(unit) ? "integer" : ["integer", "null"], minimum: 1,
             description: "Cumulative cap. Media null means explicitly authorized unlimited quantity, still metered. Omission is not authorization." }]
  }, required: UNITS }.freeze
  ID = { type: "string", format: "uuid" }.freeze
  IMAGE_RETRY_POLICY = { type: "object", additionalProperties: false,
    properties: { mode: { type: "string", enum: ["per_material_v1"] },
      legacyBindings: { type: "array", items: { type: "object", additionalProperties: false,
        properties: { requestID: ID, subject: { type: "string", pattern: "^[0-9a-f]{64}$" }, fileName: { type: "string", minLength: 1 } },
        required: %w[requestID subject fileName] } } }, required: %w[mode legacyBindings],
    description: "Explicit human-approved migration to four image requests per stable material, counting every existing request. Legacy bindings must cover every old image receipt and match verified response filenames. Cannot reset counts or bypass uncertain media." }.freeze
  HISTORICAL_HOLD = { type: "object", additionalProperties: false,
    properties: { tokens: { type: "integer", minimum: 1 }, modelCalls: { type: "integer", minimum: 1 },
      episodeCalls: { type: "object", patternProperties: { "^[1-9][0-9]?$" => { type: "integer", minimum: 1 } }, additionalProperties: false },
      evidenceDigest: { type: "string", pattern: "^[0-9a-f]{64}$" }, generationCount: { type: "integer", minimum: 1 }, generationsDigest: { type: "string", pattern: "^[0-9a-f]{64}$" } },
    required: %w[tokens modelCalls episodeCalls evidenceDigest generationCount generationsDigest],
    description: "Conservative historical capacity hold from supplier reconciliation. Does not increase limits, alter receipts or assert actual cost. Bound to existing receipt identities; cannot be lowered or replaced on resume." }.freeze
  OPERATIONS = %w[start list inspect cancel resume budget pause music delivery file].freeze
  TOOLS = OPERATIONS.map do |operation|
    properties = case operation
                 when "start" then { brief: { type: "string", minLength: 1, maxLength: 4_000 }, sourceScreenplayID: ID,
                   requestID: ID, workspace: { type: "string" }, episodeCount: { type: "integer", minimum: 1, maximum: 80, default: 40 },
                   episodeSeconds: { type: "integer", minimum: 30, maximum: 300, default: 90 },
                   aspect: { type: "string", enum: %w[portrait landscape square], default: "portrait" }, limits: LIMITS,
                   maxModelCallsPerEpisode: { type: "integer", minimum: 1, description: "Hard per-episode model call cap including reviews and retries. Global planning/reviews also consume the overall model_calls cap." } }
                 when "list" then {}
                 when "inspect" then { id: ID, detail: { type: "boolean", default: false } }
                 when "budget" then { id: ID, expected: LIMITS, limits: LIMITS }
                 when "music" then { id: ID, deferred: { type: "boolean" } }
                 when "resume" then { id: ID, historicalUsageHold: HISTORICAL_HOLD, imageRetryPolicy: IMAGE_RETRY_POLICY,
                   retryUnknownImageRequests: { type: "array", minItems: 1, maxItems: 1,
                     items: { type: "object", additionalProperties: false, properties: { requestID: ID, subject: { type: "string", pattern: "^[0-9a-f]{64}$" } }, required: %w[requestID subject] },
                     description: "ONLY with separate explicit human authorization for this exact synchronous unknown image request and stable material subject, after reconciliation. Retain the complete unknown receipt and one-image reservation; allow one new image purchase, counting both against the existing four-per-material cap. Duplicate cost is possible. A replacement unknown needs new approval; other media and final delivery settlement stay protected." },
                   retryUnknownTextRequests: { type: "array", minItems: 1, maxItems: 8, uniqueItems: true, items: ID,
                   description: "ONLY with explicit human authorization for these exact unknown text request IDs. Keeps their unknown state and full reservation; opens one new episode writing or text dual-review attempt within existing caps. May incur duplicate cost. Never applies to media, running/reserved requests or final delivery settlement." } }
                 when "file" then { id: ID, selector: { type: "string", pattern: "^(manifest|records|episode-[1-9][0-9]?)$" } }
                 else { id: ID }
                 end
    description = case operation
                  when "start" then "Start a durable complete-film series from a rough topic OR a verified sourceScreenplayID. Supports 1-80 episodes, default 40, commonly 20-80. Runtime owns planning, writing, independent reviews, revisions, images, speech, music, video, quality repair, composition and delivery. Requires explicit cumulative resource limits; previous text-token grants do not authorize new media. requestID is a fresh UUID reused unchanged on retries. Do not start again to check progress."
                  when "list" then "Read allowed workspaces and production task summaries. No paid calls."
                  when "inspect" then "Read the same Runtime task's stages, per-episode progress, resource usage and domain issues. Default is compact; detail=true includes node/receipt metadata. No paid calls."
                  when "budget" then "Increase cumulative resource limits ONLY after explicit human authorization. expected must match current limits; preserve all usage, receipts and revisions; does not resume automatically."
                  when "pause" then "Pause after current generation receipts settle. Preserve the original task, content, usage and revisions; does not cancel or interrupt a paid request."
                  when "music" then "Set deferred=true when the human asks to postpone music. Keep music nodes pending; continue eligible images, speech, videos and reviews across episodes. Composition, whole-film acceptance and delivery still require music. Setting false does not resume a paused task. No budget change."
                  when "resume" then "Continue a paused production within its current limits, preserving content, usage and upstream IDs. Uncertain text requires exact retryUnknownTextRequests authorization. A synchronous unknown image requires separate exact requestID and stable-subject approval through retryUnknownImageRequests after reconciliation, allowing only one replacement within the original material cap. Other media uncertainty stays blocked. All retained unknown reservations remain held and never count as settled delivery."
                  when "delivery" then "List freshly verified final film files, SHA256 and streaming download routes. Requires completed Runtime, full audiovisual acceptance and settled usage. No media bytes returned through MCP."
                  when "file" then "Resolve one accepted final file descriptor for the Rust streaming host. Accepts only manifest or episode-N, never a path; rechecks current Runtime and domain acceptance. No media bytes."
                  else "Stop future production dispatch. Already submitted work may still settle; preserve all existing content and usage."
                  end
    required = operation == "start" ? %w[requestID limits] : operation == "list" ? [] : operation == "budget" ? %w[id expected limits] : operation == "music" ? %w[id deferred] : operation == "file" ? %w[id selector] : ["id"]
    { name: "production.#{operation}", description: description, annotations: { readOnlyHint: %w[list inspect delivery file].include?(operation) },
      inputSchema: { type: "object", additionalProperties: false, properties: properties, required: required } }
  end.freeze

  def initialize(host:, service:)
    @host, @service = host, service
  end

  def call(operation, arguments)
    return { "ok" => false, "error" => { "code" => "production_runtime_unsupported" } } unless @host.supports?(METHOD)
    raise ArgumentError, "production_operation_invalid" unless OPERATIONS.include?(operation)
    input = arguments.dup
    input["workspace"] = input.delete("_workspace") if operation == "start" && input["_workspace"]
    if operation == "file"
      raise ArgumentError, "production_delivery_selector_invalid" unless input["selector"].to_s.match?(/\A(?:manifest|records|episode-[1-9][0-9]?)\z/)
    end
    query = %w[delivery file].include?(operation) ? "inspect" : operation
    result = @host.request(METHOD, "operation" => query, "arguments" => operation == "file" ? input.slice("id") : input)
    if %w[delivery file].include?(operation) && result.is_a?(Hash) && result["ok"]
      result = @service.delivery(result.fetch("workflow"), selector: operation == "file" ? arguments["selector"] : nil)
      if result["ok"] && operation == "delivery"
        ([result["manifest"], result["records"]] + result["episodes"]).each { |file| file["url"] = "/plugin-production/#{arguments['id']}/#{file['selector']}" }
      end
      return result
    end
    if result.is_a?(Hash) && result["ok"] && result["workflow"]
      domain = @service.status(result["workflow"]["id"])
      result["workflow"]["progress"] = domain if domain["ok"]
    end
    result
  rescue HostBridge::Unsupported, HostBridge::RequestFailed, ArgumentError, KeyError => error
    { "ok" => false, "error" => { "code" => "production_control_failed", "detail" => error.class.name } }
  end
end

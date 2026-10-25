# frozen_string_literal: true

require "json"
require "digest"

# Public chat/page commands only delegate scheduling to the Rust host. No detached Ruby loop.
class ScreenplayControl
  METHOD = "willdeep/screenplay/control"
  OPERATIONS = %w[start list inspect cancel resume budget download].freeze
  ID = { type: "string", format: "uuid" }.freeze
  BUDGET = { maxCalls: { type: "integer", minimum: 12, maximum: 128, default: 64 },
             maxTokens: { type: "integer", minimum: 50_000, maximum: 2_000_000, default: 500_000 } }.freeze
  TOOLS = OPERATIONS.map do |operation|
    properties = case operation
                 when "start" then { brief: { type: "string", minLength: 1, maxLength: 4000 }, requestID: ID,
                                     workspace: { type: "string", description: "An existing host workspace; omitted when there is only one." } }.merge(BUDGET)
                 when "list" then {}
                 when "budget" then { id: ID, expectedCalls: { type: "integer" }, expectedTokens: { type: "integer" } }.merge(BUDGET)
                 when "download" then { id: ID, format: { type: "string", enum: %w[markdown json manifest] } }
                 else { id: ID }
                 end
    description = case operation
                  when "start" then "Start a durable three-episode screenplay from a rough topic. Runtime plans, writes, independently reviews, revises and exports without per-episode confirmation. Default total limit: 500000 tokens / 64 model calls; may pause before completion. Tell the user the limit. requestID must be a fresh UUID reused unchanged on transport retries; never start again to check progress."
                  when "list" then "List existing Runtime screenplay tasks, allowed workspaces and visible default budget. Does not spend tokens."
                  when "inspect" then "Read current writing/review/revision progress, usage, issues and final export for a Runtime task. Does not spend tokens."
                  when "resume" then "Resume a paused Runtime screenplay within its existing budget, preserving all usage and revisions. Never automatically retry uncertain billed calls."
                  when "budget" then "Increase the cumulative task budget ONLY when the human explicitly authorizes the new total. CAS expectedCalls/expectedTokens must match the current limits. Does not reset usage or resume automatically."
                  when "cancel" then "Cancel future screenplay steps; an already dispatched model call may still settle. Preserves all content and usage."
                  else "Read a verified final screenplay as UTF-8 text with filename and SHA256. Requires completed Runtime and fresh local final acceptance. Formats: markdown, json, manifest. Never accepts arbitrary file paths."
                  end
    required = operation == "list" ? [] : operation == "start" ? %w[brief requestID] : operation == "budget" ? %w[id expectedCalls expectedTokens maxCalls maxTokens] : operation == "download" ? %w[id format] : ["id"]
    { name: "screenplay.#{operation}", description: description,
      annotations: { readOnlyHint: %w[list inspect download].include?(operation) },
      inputSchema: { type: "object", properties: properties, required: required } }
  end.freeze

  def initialize(host:, service:)
    @host, @service = host, service
  end

  def call(operation, arguments)
    raise HostBridge::Unsupported, "screenplay_runtime_unsupported" unless @host.supports?(METHOD)
    raise ArgumentError, "Unknown screenplay operation" unless OPERATIONS.include?(operation)

    # Chat context is injected by the host; page input remains constrained by its host workspace list.
    arguments = arguments.dup
    arguments["workspace"] = arguments.delete("_workspace") if operation == "start" && arguments["_workspace"]
    query = operation == "download" ? "inspect" : operation
    result = @host.request(METHOD, { "operation" => query, "arguments" => arguments })
    return result unless result.is_a?(Hash) && result["ok"] == true
    return @service.read_final(arguments, result.fetch("workflow")) if operation == "download"

    workflow = result["workflow"]
    if workflow
      domain = @service.status("workflowID" => workflow["id"])
      workflow["progress"] = domain if domain["ok"]
    end
    result
  rescue HostBridge::Unsupported
    { "ok" => false, "error" => { "code" => "screenplay_runtime_unsupported" } }
  rescue HostBridge::RequestFailed, ArgumentError, KeyError, ScreenplayRules::Invalid, IOError, SystemCallError, JSON::ParserError => error
    { "ok" => false, "error" => { "code" => "screenplay_control_failed", "message" => error.message } }
  end
end

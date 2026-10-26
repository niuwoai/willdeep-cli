# frozen_string_literal: true

require_relative "production_text_rules"

# MCP identifies its host after services are constructed. Resolve the active
# media directory at use time, keeping file writes and audits on that same host.
module ProductionMediaRoot
  private

  def production_media_root
    value = @root.respond_to?(:call) ? @root.call : @root
    ProductionTextRules.require!(value.is_a?(String) && !value.empty?, "production_media_root_required")
    File.expand_path(value)
  end
end

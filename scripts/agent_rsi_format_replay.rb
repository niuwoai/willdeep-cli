#!/usr/bin/env ruby
# frozen_string_literal: true
require 'optparse'
require_relative 'lib/agent_rsi_format_replay'

options = {}
OptionParser.new do |p|
  p.on('--batch PATH') { |v| options[:batch] = v }
  p.on('--audit PATH') { |v| options[:audit] = v }
  p.on('--frozen-source PATH') { |v| options[:frozen_source] = v }
  p.on('--expected-parser-sha256 SHA') { |v| options[:expected_parser_sha256] = v }
  p.on('--output PATH') { |v| options[:output] = v }
end.parse!
begin
  output = options.delete(:output) or raise AgentRsiFormatReplay::Denied, 'new separate --output is required'
  replay = AgentRsiFormatReplay::Replay.new(**options)
  report = replay.prepare
  replay.write(output, report)
  puts JSON.generate(report.slice('parser_revision', 'replayed_quality_observations', 'regressed_observations',
    'original_metrics', 'posthoc_metrics', 'posthoc_rejection_reasons', 'model_requests', 'original_inputs_unchanged'))
  exit(report['validation_passed'] ? 0 : 1)
rescue AgentRsiFormatReplay::Denied, KeyError, ArgumentError, Errno::ENOENT => error
  warn "Offline format replay rejected: #{error.class.name}: #{error.message}"
  exit 2
end

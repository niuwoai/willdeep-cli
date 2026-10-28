#!/usr/bin/env ruby
# frozen_string_literal: true

# willdeep-config 的页面契约：全局默认模型、常用 token 档位、以及解析器已经接受的键。
# 只读写临时目录里的配置，不碰 ~/.willdeep。

Encoding.default_external = Encoding::UTF_8

require "tmpdir"
require "fileutils"
require "json"

ROOT = File.expand_path("../..", __dir__)
LIB = File.join(ROOT, "crates/willdeep-cli/assets/willdeep-config/server")
$LOAD_PATH.unshift(File.join(LIB, "lib"))
require File.join(LIB, "config_server.rb")

failures = []

def check(failures, label)
  yield
rescue StandardError => e
  failures << "#{label}: #{e.class}: #{e.message}"
end

def assert_equal(expected, actual, label)
  return if expected == actual
  raise "#{label}: expected #{expected.inspect}, got #{actual.inspect}"
end

SAMPLE = <<~TOML
  version = 1
  default_provider = "magpie"

  [providers.magpie]
  provider = "openai-compatible"
  model = "glm-5"
  context_window = 131072
  max_output_tokens = 32768

  [providers.local]
  provider = "openai-compatible"
  model = "qwen"

  [subagents.implementer]
  max_turns = 24
  context_window = 65536
TOML

Dir.mktmpdir("willdeep-config-test") do |dir|
  path = File.join(dir, "config.toml")
  File.write(path, SAMPLE)
  box = WilldeepConfig::Toolbox.new(config_path: path, backup_dir: File.join(dir, "backups"))
  args = { "path" => path }

  check(failures, "recognized keys") do
    snapshot = box.call("config.snapshot", args)
    unknown = snapshot["unknownKeys"].map { |entry| [entry["section"], entry["key"]] }
    assert_equal([], unknown, "unknownKeys")
    provider = snapshot["sections"].find { |section| section["name"] == "providers.magpie" }
    keys = provider["fields"].map { |field| field["key"] }
    %w[context_window max_output_tokens vision_model].each do |key|
      raise "providers missing #{key}" unless keys.include?(key)
    end
    subagent = snapshot["sections"].find { |section| section["name"] == "subagents.implementer" }
    sub_keys = subagent["fields"].map { |field| field["key"] }
    %w[max_turns model token_budget].each do |key|
      raise "subagents missing #{key}" unless sub_keys.include?(key)
    end
    window = provider["fields"].find { |field| field["key"] == "context_window" }
    labels = window["presets"].map { |preset| [preset[:label], preset[:value]] }
    assert_equal([["32K", 32_768], ["64K", 65_536], ["128K", 131_072], ["256K", 262_144], ["1M", 1_048_576]], labels, "presets")
  end

  check(failures, "default model projection") do
    snapshot = box.call("config.snapshot", args)
    root = snapshot["sections"].find { |section| section["name"] == "" }
    model = root["fields"].find { |field| field["key"] == "default_model" }
    assert_equal("glm-5", model["value"], "projected model")
    assert_equal("magpie", model["provider"], "projected provider")
    assert_equal(true, model["virtual"], "virtual")
  end

  check(failures, "saving the default model writes the provider") do
    result = box.call("config.apply", args.merge(
      "confirm" => true,
      "edits" => [
        { "op" => "set", "section" => "", "key" => "default_provider", "value" => "local" },
        { "op" => "set", "section" => "", "key" => "default_model", "value" => "qwen-next" }
      ]
    ))
    raise "apply failed: #{result['message']}" unless result["ok"]
    text = File.read(path)
    raise "default provider not switched" unless text.include?('default_provider = "local"')
    raise "model written to the wrong provider" if text.match?(/^model = "qwen-next"/) && text.split("[providers.local]").last.split(/^\[/).first !~ /qwen-next/
    local = text.split("[providers.local]", 2).last.split(/^\[/, 2).first
    magpie = text.split("[providers.magpie]", 2).last.split(/^\[/, 2).first
    raise "local model not updated: #{local}" unless local.include?('model = "qwen-next"')
    raise "magpie model changed unexpectedly" if magpie.include?("qwen-next")
  end

  check(failures, "preset value round-trips") do
    result = box.call("config.apply", args.merge(
      "confirm" => true,
      "edits" => [{ "op" => "set", "section" => "providers.magpie", "key" => "max_output_tokens", "value" => 1_048_576 }]
    ))
    raise "apply failed: #{result['message']}" unless result["ok"]
    raise "1M not stored" unless File.read(path).include?("max_output_tokens = 1048576")
  end

  check(failures, "a default model edit follows the provider already in the file") do
    fresh = File.join(dir, "follow.toml")
    File.write(fresh, SAMPLE)
    result = box.call("config.apply", {
      "path" => fresh, "confirm" => true,
      "edits" => [{ "op" => "set", "section" => "", "key" => "default_model", "value" => "glm-next" }]
    })
    raise "apply failed: #{result['message']}" unless result["ok"]
    text = File.read(fresh)
    raise "default provider was changed: #{text}" unless text.include?('default_provider = "magpie"')
    magpie = text.split("[providers.magpie]", 2).last.split(/^\[/, 2).first
    local = text.split("[providers.local]", 2).last.split(/^\[/, 2).first
    raise "model not written to the current default provider:\n#{magpie}" unless magpie.include?('model = "glm-next"')
    raise "the other provider changed:\n#{local}" if local.include?("glm-next")
  end

  check(failures, "a default model without any provider is refused") do
    bare = File.join(dir, "bare.toml")
    File.write(bare, "version = 1\n")
    result = box.call("config.apply", {
      "path" => bare, "confirm" => true,
      "edits" => [{ "op" => "set", "section" => "", "key" => "default_model", "value" => "x" }]
    })
    raise "a model with no provider was accepted" if result["ok"]
    raise "the empty file was modified" unless File.read(bare) == "version = 1\n"
  end
end

if failures.empty?
  puts "config plugin checks passed"
  exit 0
end

warn "#{failures.length} config plugin check(s) failed:"
failures.each { |failure| warn "  - #{failure}" }
exit 1

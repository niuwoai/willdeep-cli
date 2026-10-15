#!/usr/bin/env ruby
# frozen_string_literal: true
require 'minitest/autorun'
require_relative 'lib/agent_rsi_grader'
require_relative 'lib/agent_rsi_free_test_report'
AgentRsiFreeTestReport.enable('format-parser')

class AgentRsiGraderTest < Minitest::Test
  ANSWER = '{"has_defect":true,"line":3,"reason":"Zero is incorrectly replaced"}'
  CASES = {
    'plain answer' => [ANSWER, true],
    'inline answer' => ["`#{ANSWER}`", true],
    'fenced answer' => ["```json\n#{ANSWER}\n```", true],
    'normal fenced source before answer' => ["```ts\nfunction f() { return {id:'a'}; }\n```\n#{ANSWER}", true],
    'unlabelled numbered source' => ["```\n2 function f() {\n3   return 0;\n4 }\n```\n#{ANSWER}", true],
    'tilde fenced source' => ["~~~typescript\nfunction f() { return {}; }\n~~~\n#{ANSWER}", true],
    'long fence enclosing backticks' => ["````ts\nconst x = `value`;\nfunction f() { return {}; }\n````\n#{ANSWER}", true],
    'closed partial code excerpt' => ["`if (x) {`\n#{ANSWER}", true],
    'balanced inline code' => ["#{ANSWER}\n`if (x) { return {}; }`", true],
    'inline object literal' => ["#{ANSWER}\n`uniqueIndex([{id:'a'}, {id:'a'}])`", true],
    'inline empty object' => ["#{ANSWER}\n`{}`", true],
    'balanced prose source' => ["#{ANSWER}\nfunction f() { return 0; }", true],
    'quoted abbreviated template' => ["#{ANSWER}\n`{\"has_defect\":true,\"line\":3,...}`", true],
    'quoted complete placeholder' => ["`{\"has_defect\":true,\"line\":3,\"reason\":\"...\"}`\n#{ANSWER}", true],
    'unicode placeholder' => ["#{ANSWER}\n`{\"has_defect\":true,\"line\":3,\"reason\":\"…\"}`", true],
    'unquoted inline reason placeholder' => ["#{ANSWER}\n`{\"has_defect\":true,\"line\":3,\"reason\":...}`", true],
    'JS interpolation' => ["#{ANSWER}\n```${account}:${record}```", true],
    'JS interpolation inside nested ticks' => ["#{ANSWER}\n``key = `${account}:${record}` ``", true],
    'unclosed nested tick delimiter' => ["#{ANSWER}\n``key = `${account}:${record}```", false],
    'JS interpolation does not hide full answer' => ["#{ANSWER}\n``key = `${#{ANSWER}}` ``", false],
    'placeholder not an extra field escape' => ["#{ANSWER}\n`{\"has_defect\":true,\"line\":3,\"reason\":...,\"extra\":1}`", false],
    'unquoted placeholder alone' => ['`{"has_defect":true,"line":3,"reason":...}`', false],
    'unquoted placeholder outside inline' => ["#{ANSWER}\n{\"has_defect\":true,\"line\":3,\"reason\":...}", false],
    'duplicate keys with bare reason placeholder' => ["#{ANSWER}\n`{\"has_defect\":false,\"has_defect\":true,\"line\":3,\"reason\":...}`", false],
    'template alone' => ['`{"has_defect":true,"line":3,...}`', false],
    'placeholder alone' => ['`{"has_defect":true,"line":3,"reason":"..."}`', false],
    'unquoted abbreviated answer' => ["#{ANSWER}\n{\"has_defect\":true,\"line\":3,...}", false],
    'JSON fence abbreviated answer' => ["#{ANSWER}\n```json\n{\"has_defect\":true,\"line\":3,...}\n```", false],
    'two identical complete answers' => ["#{ANSWER}\n`#{ANSWER}`", false],
    'two conflicting complete answers' => [ANSWER + '{"has_defect":false,"line":null,"reason":"clean"}', false],
    'answer hidden inside code' => ["#{ANSWER}\n```ts\nif(x) { #{ANSWER} }\n```", false],
    'answer hidden in JSON code container' => ["#{ANSWER}\n```js\n{\"wrapper\":#{ANSWER}}\n```", false],
    'answer hidden in nested array code container' => ["#{ANSWER}\n```js\n{\"wrapper\":[{\"nested\":#{ANSWER}}]}\n```", false],
    'ordinary nested JSON source data' => ["#{ANSWER}\n```js\n{\"meta\":{\"count\":1}}\n```", true],
    'duplicate keys' => ['{"has_defect":false,"has_defect":true,"line":3,"reason":"why"}', false],
    'duplicate keys with placeholder' => ["#{ANSWER}\n`{\"has_defect\":false,\"has_defect\":true,\"line\":3,\"reason\":\"...\"}`", false],
    'duplicate abbreviated keys' => ["#{ANSWER}\n`{\"has_defect\":false,\"has_defect\":true,\"line\":3,...}`", false],
    'unknown malformed object' => [ANSWER + '`{broken}`', false],
    'JSON-looking malformed code' => ["#{ANSWER}\n```js\n{\"has_defect\":true,}\n```", false],
    'truncated JSON answer' => [ANSWER + '`{"has_defect":', false],
    'unclosed code fence' => ["#{ANSWER}\n```js\nif(x) {", false],
    'unclosed inline excerpt' => [ANSWER + '`if(x) {', false],
    'unclosed prose excerpt' => [ANSWER + ' if(x) {', false],
    'wrong schema fields' => ['{"has_defect":true,"line":3,"reason":"why","passed":true}', false],
    'string boolean' => ['{"has_defect":"true","line":3,"reason":"why"}', false],
    'float location' => ['{"has_defect":true,"line":3.0,"reason":"why"}', false],
    'wrong shape beside answer' => [ANSWER + '{"has_defect":true}', false]
  }.freeze

  def grade(raw, **gates)
    AgentRsiLive::Grader.grade(raw, {'has_defect' => true, 'line' => 3},
      read_observed: gates.fetch(:read_observed, true), fixture_unchanged: gates.fetch(:fixture_unchanged, true))
  end

  CASES.each do |name, (raw, expected)|
    define_method('test_' + name.gsub(/\W+/, '_')) { assert_equal expected, grade(raw)['passed'], name }
  end

  def test_braces_backticks_and_escaped_quotes_in_reason_are_literal
    reason = "Use `for (;;) {` and ```json and escaped \"quotes\" / \\\\ and {x} in evidence."
    raw = JSON.generate('has_defect' => true, 'line' => 3, 'reason' => reason)
    assert_equal reason, AgentRsiLive::Grader.objects(raw).first['reason']
    assert grade(raw)['passed']
    assert grade("```json\n#{raw}\n```")['passed']
    assert grade("`#{raw}`")['passed']
  end

  def test_extraction_does_not_use_oracle_to_pick_an_answer
    first = grade(ANSWER + '{"has_defect":false,"line":null,"reason":"clean"}')
    second = AgentRsiLive::Grader.grade(ANSWER + '{"has_defect":false,"line":null,"reason":"clean"}',
      {'has_defect' => false, 'line' => nil}, read_observed: true, fixture_unchanged: true)
    refute first['schema_valid']
    refute second['schema_valid']
    assert_equal first['format_diagnostic'], second['format_diagnostic']
  end

  def test_no_read_or_unchanged_fixture_credit_from_answer
    refute grade(ANSWER, read_observed: false)['passed']
    refute grade(ANSWER, fixture_unchanged: false)['passed']
  end

  def test_classifier_and_location_remain_independent_of_format
    result = grade(ANSWER.sub('"line":3', '"line":4'))
    assert result['schema_valid']
    assert result['classification_correct']
    refute result['localization_correct']
    assert_equal 'report-json-v5', result['parser_revision']
  end

  def test_grader_does_not_load_network_or_paid_runner
    refute $LOADED_FEATURES.any? { |p| p.match?(/net\/http|agent_rsi_live(?:_account|_host)?\.rb/) }
  end
end

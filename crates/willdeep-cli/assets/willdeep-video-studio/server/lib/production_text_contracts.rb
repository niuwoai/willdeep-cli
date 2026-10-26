# frozen_string_literal: true

# Model-facing structures mirror the deterministic validators. Validation and
# source-evidence checks remain authoritative; schemas do not manufacture text.
module ProductionTextContracts
  module_function

  def text(maximum)
    { "type" => "string", "minLength" => 1, "maxLength" => maximum }
  end

  def object(properties)
    { "type" => "object", "properties" => properties, "required" => properties.keys, "additionalProperties" => false }
  end

  def array(items, minimum, maximum)
    { "type" => "array", "items" => items, "minItems" => minimum, "maxItems" => maximum }
  end

  def outline(expected, plan)
    names = plan.fetch("characters").map { |entry| entry.fetch("name") }
    reveal = object("id" => { "enum" => plan.fetch("facts").map { |entry| entry.fetch("id") } },
      "to" => array({ "enum" => names }, 1, names.length))
    episode = object({ "order" => { "type" => "integer", "enum" => expected } }.merge(
      %w[title summary entry events exit].to_h { |key| [key, text(2_000)] }).merge("reveals" => array(reveal, 0, 12)))
    object("episodes" => array(episode, expected.length, expected.length))
  end

  def review(coverage)
    issue = object(%w[rule excerpt reason suggestion].to_h { |key| [key, text(1_000)] }.merge(
      "episodeOrder" => { "type" => "integer", "enum" => coverage },
      "level" => { "enum" => %w[must advice] }, "severity" => { "enum" => %w[warn block] }))
    object("status" => { "enum" => %w[pass warn block] }, "summary" => text(1_500),
      "coverage" => { "type" => "array", "const" => coverage }, "issues" => array(issue, 0, 30),
      "resolved" => array(object("id" => text(80), "evidence" => text(1_000)), 0, 30))
  end

  def episode(order, spec, plan, available_facts)
    names = plan.fetch("characters").map { |entry| entry.fetch("name") }
    scene_id = text(80).merge("pattern" => "^e#{order}s[1-9][0-9]*$")
    evidence = { "sceneID" => scene_id, "excerpt" => text(1_000) }
    action = object("action" => text(3_000))
    dialogue = object("speaker" => { "enum" => names }, "text" => text(600))
    scene = object("id" => scene_id, "heading" => text(160), "beats" => array({ "oneOf" => [action, dialogue] }, 2, 80))
    fact = object({ "id" => text(80).merge("pattern" => "^e#{order}f[1-9][0-9]*$"), "text" => text(600),
      "knownBy" => array({ "enum" => names }, 0, names.length) }.merge(evidence))
    reveal = object({ "id" => { "enum" => plan.fetch("facts").map { |entry| entry.fetch("id") } },
      "to" => array({ "enum" => names }, 1, names.length) }.merge(evidence))
    variants = plan.fetch("threads").map do |entry|
      object({ "id" => { "enum" => [entry.fetch("id")] },
        "event" => { "enum" => thread_events(order, entry) } }.merge(evidence))
    end
    thread = variants.empty? ? object(evidence.merge("id" => { "enum" => [] }, "event" => { "enum" => [] })) : { "oneOf" => variants }
    full = object("title" => text(160), "summary" => text(2_000), "scenes" => array(scene, 2, [20, spec.fetch("episode_seconds") / 4].min),
      "events" => array(object({ "summary" => text(600) }.merge(evidence)), 1, 12), "newFacts" => array(fact, 0, 4),
      "reveals" => array(reveal, 0, 12), "threadEvents" => array(thread, 0, 16))
    return full if available_facts.empty?
    lookup = object("needFacts" => array({ "enum" => available_facts.map { |entry| entry.fetch("id") } }, 1, 8))
    { "oneOf" => [full, lookup] }
  end

  def thread_events(order, thread)
    events = ["reinforce"]
    events << "plant" if thread.fetch("plantEpisode") == order
    events << "payoff" if thread.fetch("payoffEpisode") == order
    events
  end
end

# frozen_string_literal: true

require_relative "screenplay_rules"

module ProductionTextRules
  Invalid = ScreenplayRules::Invalid
  MIN_EPISODE_BEATS = 6
  EVIDENCE_SCENE_LIMIT = 20_000
  class EvidenceInvalid < Invalid
    attr_reader :repair_material

    def initialize(errors, scenes)
      first = errors.first
      source = scenes.select { |scene| errors.any? { |error| error["sceneID"] == scene["id"] } }
      @repair_material = first.merge("errors" => errors,
        "renderedScene" => ScreenplayRules.render("scenes" => source.select { |scene| scene["id"] == first["sceneID"] })[0, EVIDENCE_SCENE_LIMIT],
        "renderedScenes" => ScreenplayRules.render("scenes" => source)[0, EVIDENCE_SCENE_LIMIT],
        "availableSceneIDs" => scenes.map { |scene| scene["id"] },
        "instruction" => "逐项修正errors中的全部证据。excerpt必须是对应sceneID的连续逐字子串，不能借用别的场次。保持已有效的正文场次不变，从正文重新复制真实证据。" )
      super("production_evidence_not_in_scene: #{first['field']}[#{first['index']}] sceneID=#{first['sceneID']}; invalidCount=#{errors.length}")
    end
  end
  module_function

  def require!(condition, message)
    ScreenplayRules.require!(condition, message)
  end

  def text!(value, label, maximum = 4_000)
    ScreenplayRules.text!(value, label, maximum)
  end

  def range(spec)
    (1..spec.fetch("episode_count")).each_slice(8).map { |orders| { "first" => orders.first, "last" => orders.last } }
  end

  def plan!(value, spec)
    require!(value.is_a?(Hash), "production_plan_object_required")
    { "title" => 160, "genre" => 80, "logline" => 1_200, "coreConflict" => 1_200,
      "arc" => 4_000, "tone" => 300, "audience" => 300, "visualStyle" => 300, "ending" => 4_000 }
      .each { |key, maximum| text!(value[key], key, maximum) }
    cast = value["characters"]
    require!(cast.is_a?(Array) && cast.length.between?(2, 12) && cast.all? { |entry| entry.is_a?(Hash) }, "production_cast_required")
    cast.each do |entry|
      text!(entry["name"], "character name", 120)
      text!(entry["role"], "character biography", 2_000)
      text!(entry["visualPrompt"], "character visual prompt", 4_000)
    end
    names = cast.map { |entry| entry["name"] }
    require!(names.uniq == names, "production_duplicate_character")
    facts = value["facts"]
    require!(facts.is_a?(Array) && facts.length.between?(1, 24), "production_locked_facts_required")
    facts.each do |fact|
      require!(fact.is_a?(Hash), "production_fact_object_required")
      text!(fact["id"], "fact ID", 80)
      text!(fact["text"], "fact text", 600)
      require!(fact["knownBy"].is_a?(Array) && (fact["knownBy"] - names).empty?, "production_fact_knowledge_invalid")
    end
    require!(facts.map { |fact| fact["id"] }.uniq.length == facts.length, "production_duplicate_fact")
    arcs = value["arcs"]
    require!(arcs.is_a?(Array) && arcs.all? { |arc| arc.is_a?(Hash) } &&
      arcs.map { |arc| arc.slice("first", "last") } == range(spec), "production_arc_coverage_invalid")
    arcs.each { |arc| %w[goal turn exit].each { |key| text!(arc[key], "arc #{key}", 1_500) } }
    threads = value["threads"]
    require!(threads.is_a?(Array) && threads.length.between?(1, 16), "production_threads_required")
    threads.each do |thread|
      require!(thread.is_a?(Hash), "production_thread_object_required")
      text!(thread["id"], "thread ID", 80)
      text!(thread["promise"], "thread promise", 600)
      first, last = thread["plantEpisode"], thread["payoffEpisode"]
      require!(first.is_a?(Integer) && last.is_a?(Integer) && first.between?(1, spec["episode_count"]) &&
        last.between?(first, spec["episode_count"]), "production_thread_timeline_invalid")
    end
    require!(threads.map { |entry| entry["id"] }.uniq.length == threads.length, "production_duplicate_thread")
    value
  end

  def outlines!(value, expected, plan)
    episodes = value.is_a?(Hash) && value["episodes"]
    require!(episodes.is_a?(Array) && episodes.all? { |entry| entry.is_a?(Hash) } &&
      episodes.map { |entry| entry["order"] } == expected, "production_outline_coverage_invalid")
    episodes.each do |entry|
      %w[title summary entry events exit].each { |key| text!(entry[key], "outline #{key}", 2_000) }
      require!(!entry.key?("script"), "production_outline_cannot_prefill_script")
      reveals!(entry["reveals"], plan)
    end
    episodes
  end

  def reveals!(reveals, plan)
    require!(reveals.is_a?(Array) && reveals.length <= 12, "production_reveals_invalid")
    names = plan["characters"].map { |entry| entry["name"] }
    reveals.each do |reveal|
      require!(reveal.is_a?(Hash) && plan["facts"].any? { |fact| fact["id"] == reveal["id"] } &&
        reveal["to"].is_a?(Array) && !reveal["to"].empty? && (reveal["to"] - names).empty?, "production_reveal_invalid")
    end
  end

  def episode!(value, order, spec, plan, contract)
    require!(value.is_a?(Hash), "production_episode_object_required")
    text!(value["title"], "episode title", 160)
    text!(value["summary"], "episode summary", 2_000)
    scenes = value["scenes"]
    require!(scenes.is_a?(Array) && scenes.length.between?(2, 20) && scenes.all? { |scene| scene.is_a?(Hash) }, "production_complete_scenes_required")
    require!(scenes.length * 4 <= spec["episode_seconds"], "production_too_many_scenes_for_duration")
    require!(scenes.map { |scene| scene["id"] }.uniq.length == scenes.length, "production_duplicate_scene")
    names = plan["characters"].map { |entry| entry["name"] }
    beat_count = 0
    scenes.each do |scene|
      require!(scene["id"].is_a?(String) && scene["id"].match?(/\Ae#{order}s[1-9][0-9]*\z/), "production_stable_scene_id_required")
      text!(scene["heading"], "scene heading", 160)
      ScreenplayRules.beats!(scene["beats"], names, allow_silent: true)
      beat_count += scene["beats"].length
    end
    require!(beat_count >= MIN_EPISODE_BEATS, "production_complete_beats_required: expected at least #{MIN_EPISODE_BEATS} ordered dramatic beats, received #{beat_count}")
    script = ScreenplayRules.render(value)
    minimum = [[spec["episode_seconds"] * 3, 100].max, 900].min
    require!(script.length.between?(minimum, 20_000) && !script.match?(/TODO|此处省略|待补充/), "production_complete_script_required")
    errors = { "events" => 12, "reveals" => 12, "newFacts" => 4, "threadEvents" => 16 }.flat_map do |field, maximum|
      evidence_errors(value[field], scenes, script, maximum, field)
    end
    raise EvidenceInvalid.new(errors, scenes) unless errors.empty?
    evidence!(value["events"], scenes, script, 12, field: "events")
    require!(!value["events"].empty?, "production_causal_events_required")
    value["events"].each { |event| text!(event["summary"], "causal event summary", 600) }
    new_facts!(value["newFacts"], scenes, script, order, names)
    reveals!(value["reveals"], plan)
    require!(value["reveals"].map { |reveal| reveal.slice("id", "to") } == contract["reveals"], "production_unplanned_knowledge_change")
    evidence!(value["reveals"], scenes, script, 12, field: "reveals")
    thread_events!(value["threadEvents"], scenes, script, order, plan)
    value
  end

  def evidence!(entries, scenes, script, maximum, field: "evidence")
    errors = evidence_errors(entries, scenes, script, maximum, field)
    raise EvidenceInvalid.new(errors, scenes) unless errors.empty?
  end

  def evidence_errors(entries, scenes, script, maximum, field)
    require!(entries.is_a?(Array) && entries.length <= maximum, "production_evidence_list_invalid")
    entries.each_with_index.map do |entry, index|
      require!(entry.is_a?(Hash), "production_evidence_object_required")
      text!(entry["excerpt"], "evidence excerpt", 1_000)
      scene = scenes.find { |item| item["id"] == entry["sceneID"] }
      rendered = scene ? ScreenplayRules.render("scenes" => [scene]) : nil
      next if rendered && rendered.include?(entry["excerpt"]) && script.include?(entry["excerpt"])
      { "field" => field, "index" => index, "sceneID" => entry["sceneID"], "invalidExcerpt" => entry["excerpt"] }
    end.compact
  end

  def new_facts!(facts, scenes, script, order, names)
    evidence!(facts, scenes, script, 4, field: "newFacts")
    facts.each do |fact|
      require!(fact["id"].is_a?(String) && fact["id"].match?(/\Ae#{order}f[1-9][0-9]*\z/), "production_fact_id_invalid")
      text!(fact["text"], "new fact", 600)
      require!(fact["knownBy"].is_a?(Array) && (fact["knownBy"] - names).empty?, "production_fact_knowledge_invalid")
    end
    require!(facts.map { |entry| entry["id"] }.uniq.length == facts.length, "production_duplicate_fact")
  end

  def thread_events!(events, scenes, script, order, plan)
    evidence!(events, scenes, script, 16, field: "threadEvents")
    events.each do |event|
      thread = plan["threads"].find { |entry| entry["id"] == event["id"] }
      require!(thread && %w[plant reinforce payoff].include?(event["event"]), "production_thread_event_invalid")
      require!(event["event"] != "payoff" || thread["payoffEpisode"] == order,
        "production_premature_payoff: thread=#{thread['id']}; currentEpisode=#{order}; payoffEpisode=#{thread['payoffEpisode']}; non-milestone setup or echoes use reinforce")
      require!(event["event"] != "plant" || thread["plantEpisode"] == order,
        "production_thread_plant_invalid: thread=#{thread['id']}; currentEpisode=#{order}; plantEpisode=#{thread['plantEpisode']}; non-milestone setup or echoes use reinforce")
    end
    plan["threads"].each do |thread|
      %w[plant payoff].each do |kind|
        due = thread[kind == "plant" ? "plantEpisode" : "payoffEpisode"] == order
        require!(!due || events.any? { |event| event["id"] == thread["id"] && event["event"] == kind }, "production_missing_#{kind}_evidence")
      end
    end
  end
end

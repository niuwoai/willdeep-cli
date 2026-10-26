# frozen_string_literal: true

require "digest"
require "json"

# 三集试点的确定性内容门禁。模型意见不能代替这些校验。
module ScreenplayRules
  EPISODES = 3
  MIN_SCRIPT = 450
  MAX_SCRIPT = 12_000
  STEPS = %w[plan review_plan revise_plan write_episode review_episode revise_episode final_review export].freeze
  Invalid = Class.new(StandardError)
  module_function

  def require!(condition, message)
    raise Invalid, message unless condition
  end

  def text!(value, label, max = 4_000)
    require!(value.is_a?(String) && !value.strip.empty? && value.length <= max,
      "Invalid #{label}: expected a non-empty string of at most #{max} characters; received #{value.class}#{value.is_a?(String) ? " (#{value.length} characters)" : ''}")
  end

  def digest(value)
    Digest::SHA256.hexdigest(JSON.generate(value))
  end

  def plan!(value)
    require!(value.is_a?(Hash), "Plan must be an object")
    concepts = value["concepts"]
    require!(concepts.is_a?(Array) && concepts.length == 3, "Compare three concepts")
    require!(concepts.all? { |entry| entry.is_a?(Hash) }, "Concepts must be objects")
    concepts.each { |entry| text!(entry["logline"], "concept logline") }
    require!(value["selected"].is_a?(Integer) && (0..2).include?(value["selected"]), "Invalid selected concept")
    text!(value["decision"], "concept decision")
    plan = value["plan"]
    require!(plan.is_a?(Hash) && CreativeSchema.valid?("plan", plan), "Invalid creative plan")
    %w[title logline coreConflict arc tone audience].each { |key| text!(plan[key], key) }
    require!(plan["episodeCount"] == EPISODES && Array(plan["episodes"]).length == EPISODES, "Exactly three episodes required")
    Array(plan["episodes"]).each do |episode|
      text!(episode["title"], "episode title", 160)
      text!(episode["summary"], "episode summary", 2_000)
      require!(episode["script"].to_s.empty?, "Planning cannot prefill scripts")
    end
    cast = plan["characters"]
    require!(cast.is_a?(Array) && cast.length.between?(2, 6), "Choose 2-6 named characters")
    require!(cast.all? { |entry| entry.is_a?(Hash) }, "Characters must be objects")
    names = cast.map { |character| character["name"] }
    require!(names.uniq.length == names.length, "Duplicate character names")
    cast.each { |character| text!(character["name"], "character name", 120); text!(character["role"], "character biography", 2_000) }
    bible = value["bible"]
    require!(bible.is_a?(Hash), "Missing story bible")
    text!(bible["ending"], "ending")
    require!(bible["facts"].is_a?(Array) && !bible["facts"].empty?, "Missing locked story facts")
    bible["facts"].each { |fact| text!(fact, "story fact", 500) }
    contracts = value["contracts"]
    require!(contracts.is_a?(Array) && contracts.all? { |entry| entry.is_a?(Hash) } && contracts.map { |entry| entry["order"] } == [1, 2, 3], "Episode contracts must be ordered 1-3")
    contracts.each { |contract| %w[entry events exit].each { |key| text!(contract[key], "contract #{key}") } }
    threads = value["threads"]
    require!(threads.is_a?(Array) && threads.length.between?(1, 8), "Define 1-8 important plot threads")
    require!(threads.all? { |entry| entry.is_a?(Hash) }, "Threads must be objects")
    threads.each do |thread|
      text!(thread["id"], "thread ID", 80)
      text!(thread["promise"], "thread promise", 500)
      require!(thread["plantEpisode"].is_a?(Integer) && thread["payoffEpisode"].is_a?(Integer) &&
        thread["plantEpisode"].between?(1, 3) && thread["payoffEpisode"].between?(thread["plantEpisode"], 3), "Invalid thread timeline")
    end
    require!(threads.map { |entry| entry["id"] }.uniq.length == threads.length, "Duplicate thread IDs")
    value
  end

  def candidate!(value, order, cast)
    require!(value.is_a?(Hash), "Candidate must be an object")
    text!(value["title"], "episode title", 160)
    text!(value["summary"], "episode summary", 2_000)
    scenes = value["scenes"]
    require!(scenes.is_a?(Array) && scenes.length.between?(2, 6), "Write 2-6 complete scenes")
    require!(scenes.all? { |entry| entry.is_a?(Hash) }, "Scenes must be objects")
    require!(scenes.map { |scene| scene["id"] }.uniq.length == scenes.length, "Duplicate scene IDs")
    lines = 0
    scenes.each do |scene|
      require!(scene["id"].to_s.match?(/\Ae#{order}s[1-9][0-9]*\z/), "Stable scene ID must start with e#{order}s")
      text!(scene["heading"], "scene heading", 160)
      if scene.key?("beats")
        lines += beats!(scene["beats"], cast)
        next
      end
      text!(scene["action"], "visible scene action", 3_000)
      dialogue = scene["dialogue"]
      require!(dialogue.is_a?(Array) && !dialogue.empty? && dialogue.length <= 40, "Missing scene dialogue")
      require!(dialogue.all? { |entry| entry.is_a?(Hash) }, "Dialogue lines must be objects")
      dialogue.each do |line|
        require!(cast.include?(line["speaker"]), "Unknown speaker: #{line['speaker']}")
        text!(line["text"], "dialogue", 600)
      end
      lines += dialogue.length
    end
    require!(lines >= 6, "Script needs at least six dialogue lines")
    script = render(value)
    require!(script.length.between?(MIN_SCRIPT, MAX_SCRIPT), "Complete script must contain #{MIN_SCRIPT}-#{MAX_SCRIPT} characters")
    require!(!script.match?(/TODO|待续正文|此处省略|待补充/), "Placeholder script refused")
    value
  end

  def render(candidate)
    candidate.fetch("scenes").map do |scene|
      beats = if scene["beats"]
                scene["beats"].map { |beat| beat.key?("action") ? beat["action"] : "#{beat['speaker']}：#{beat['text']}" }
              else
                [scene["action"], *scene["dialogue"].map { |line| "#{line['speaker']}：#{line['text']}" }]
              end
      ["[#{scene['id']}] #{scene['heading']}", *beats].join("\n")
    end.join("\n\n")
  end

  def beats!(beats, cast, allow_silent: false)
    require!(beats.is_a?(Array) && beats.length.between?(2, 80), "Scene needs ordered action and dialogue beats")
    dialogue = 0
    actions = 0
    beats.each do |beat|
      require!(beat.is_a?(Hash), "Beat must be an object")
      if beat.key?("action")
        require!((beat.keys - ["action"]).empty?, "Action beat cannot also contain dialogue")
        text!(beat["action"], "visible beat action", 3_000)
        actions += 1
      else
        require!((beat.keys - %w[speaker text]).empty? && cast.include?(beat["speaker"]), "Unknown beat speaker")
        text!(beat["text"], "beat dialogue", 600)
        dialogue += 1
      end
    end
    require!(actions.positive?, "Each scene needs visible action")
    require!(allow_silent || dialogue.positive?, "Each scene needs visible action and dialogue")
    dialogue
  end

  def review!(value, source, coverage)
    require!(value.is_a?(Hash) && %w[pass warn block].include?(value["status"]), "Invalid review status")
    require!(value["coverage"] == coverage, "Review coverage must equal #{coverage.to_json}; received #{value['coverage'].to_json}. Context episodes are not review targets")
    text!(value["summary"], "review summary", 1_500)
    require!(value["issues"].is_a?(Array) && value["issues"].length <= 30, "Invalid review issues")
    require!(value["issues"].all? { |entry| entry.is_a?(Hash) }, "Issues must be objects")
    value["issues"].each_with_index do |issue, index|
      %w[rule reason suggestion excerpt].each { |key| text!(issue[key], "issue #{key}", 1_000) }
      require!(%w[must advice].include?(issue["level"]) && %w[warn block].include?(issue["severity"]), "Issue severity and level must be explicit")
      require!(source.include?(issue["excerpt"]), "Review excerpt is absent from its source at issues[#{index}].excerpt. Copy a contiguous substring from source exactly; do not add speaker labels, quotes, ellipses or fact metadata.")
      require!(coverage.include?(issue["episodeOrder"]), "Issue episodeOrder must be one of #{coverage.to_json}; received #{issue['episodeOrder'].to_json}. 0 means the whole outline; episode numbers quoted inside the source are context, not targets. Preserve the finding and its original evidence.")
    end
    require!(value["status"] != "pass" || value["issues"].none? { |issue| mandatory?(issue) }, "A pass cannot hide mandatory issues")
    require!(value["status"] != "block" || value["issues"].any? { |issue| mandatory?(issue) }, "A block requires an actionable mandatory issue")
    Array(value["resolved"]).each do |entry|
      text!(entry["id"], "resolved issue ID", 80)
      text!(entry["evidence"], "resolution evidence", 1_000)
      require!(source.include?(entry["evidence"]), "Resolution evidence is absent from current content")
    end
    value
  end

  def mandatory?(issue)
    issue["level"] == "must" || issue["severity"] == "block"
  end
end

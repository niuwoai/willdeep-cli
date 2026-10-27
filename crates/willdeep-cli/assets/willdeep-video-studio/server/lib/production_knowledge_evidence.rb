# frozen_string_literal: true

module ProductionKnowledgeEvidence
  SEMANTICS = [
    "knowledgeIndex只是历史证据编号索引，不证明角色已知该事实定义的所有分句。策划facts.knownBy表示开场知情；后续知情范围必须对照planFactDisclosures的真实集号、接收者和逐字披露证据，未被披露的分句不能自动补成已知。已知角色仍可盘问、试探、隐瞒或在本集主动公开自己知道的信息，不能仅凭疑问句判定失忆，也不能因过去隐瞒就禁止后来承认。",
    "已验收前情中明确披露的知情事实不能被后续契约里的‘第一次’‘终于’‘说开’等转折用语重置。这些转折要结合既有知情解释为首次主动解释动机、交代新细节或关系变化，不能为了契约词面把已知人物改成不知道。仍须保留实际新信息与真实因果缺陷，不能虚构失忆或改写已验收前情。",
    "currentMetadata是本次候选的结构化证据，knownBy/to只作用于它所在的那条事实或披露，不能从另一条的知情者推断本条。旧意见描述的字段值必须逐项对照当前ID和当前值；正文摘录相同不等于元数据没有修正。"
  ].join("\n").freeze

  private

  def grounded_story_state(series, order, adopted = nil)
    prior = prompt_story_state(series["storyStates"][(order - 1).to_s])
    return unless prior
    adopted ||= series["episodes"].select { |number, node| number.to_i < order && node["adoptedBasis"] }
    prior["knowledgeIndex"] = prior.delete("knowledge")
    prior["knowledgeIndexSemantics"] = SEMANTICS
    prior["planFactDisclosures"] = adopted.flat_map do |number, node|
      candidate = node.fetch("candidate")
      basis = ScreenplayRules.digest(candidate)
      ProductionTextRules.require!(node["adoptedBasis"] == basis, "production_knowledge_source_changed")
      candidate.fetch("reveals").map do |reveal|
        scene = candidate.fetch("scenes").find { |entry| entry["id"] == reveal["sceneID"] }
        source = scene && ScreenplayRules.render("scenes" => [scene])
        ProductionTextRules.require!(source && reveal["excerpt"].is_a?(String) && !reveal["excerpt"].empty? &&
          source.include?(reveal["excerpt"]), "production_knowledge_evidence_invalid")
        reveal.slice("id", "to", "sceneID", "excerpt").merge("episodeOrder" => number.to_i, "sourceBasis" => basis)
      end
    end
    prior
  end
end

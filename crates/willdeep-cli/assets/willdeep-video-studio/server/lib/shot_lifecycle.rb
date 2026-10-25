# frozen_string_literal: true

# Removing a shot changes the active timeline, never its media or job ledger.
# Full snapshots, including unadopted drafts, remain in the episode archive.
module ShotLifecycle
  ACTIVE_SHOT_STATES = %w[queued running submitting in_progress unknown cancel_requested].freeze
  SHOT_REVISION_FIELDS = %w[expectedEpisodeRev expectedEpisodeDraftRev expectedRev expectedDraftRev].freeze

  def self.tools
    properties = %w[dramaID episodeID shotID].to_h { |key| [key, { type: "string" }] }
    SHOT_REVISION_FIELDS.each { |key| properties[key] = { type: "integer", minimum: 0 } }
    descriptions = {
      "remove" => "Remove one shot by immutable ID from the active timeline, keeping its complete snapshot (draft, references, selected media and QA) in episode.removedShots. Renumber remaining shots without changing their IDs or durations. Never delete media or video jobs. Refuses while this episode has active work. Does not rewrite the script or redistribute duration: inspect the returned durationSeconds and re-review the storyboard before production.",
      "restore" => "Restore a removed shot by immutable ID from episode.removedShots at its former position (or the end), preserving draft, references, media and QA. Renumber active shots and report durationSeconds. Requires current episode guards and the archived shot's rev/draftRev. Refuses while this episode has active work."
    }
    descriptions.map do |action, description|
      { name: "drama.#{action}_shot", description: description,
        inputSchema: { type: "object", properties: properties, required: properties.keys } }
    end
  end

  attr_writer :shot_lifecycle_activity

  def remove_shot(arguments)
    change_shot_membership(arguments, restore: false)
  end

  def restore_shot(arguments)
    change_shot_membership(arguments, restore: true)
  end

  private

  def change_shot_membership(arguments, restore:)
    unless SHOT_REVISION_FIELDS.all? { |key| arguments[key].is_a?(Integer) && arguments[key] >= 0 }
      return failure("invalid_revision", "Episode and shot revision guards are required.")
    end
    guarded do
      changed_id = arguments["shotID"].to_s
      drama = mutate_episode(arguments) do |episode, _item|
        check_revision(episode, arguments["expectedEpisodeRev"])
        check_revision(episode, arguments["expectedEpisodeDraftRev"], "draftRev")
        assert_shot_timeline_idle!(arguments)
        episode["removedShots"] ||= []
        if restore
          restore_archived_shot!(episode, arguments)
        else
          remove_active_shot!(episode, arguments)
        end
        renumber_active_shots!(episode)
        episode["rev"] = episode["rev"].to_i + 1
      end
      episode = drama["episodes"].find { |entry| entry["id"] == arguments["episodeID"] }
      { "ok" => true, "drama" => drama, "shotID" => changed_id, "restored" => restore,
        "shotCount" => episode["shots"].length,
        "durationSeconds" => episode["shots"].sum { |shot| shot["duration"].to_i } }
    end
  end

  def assert_shot_timeline_idle!(arguments)
    return unless @shot_lifecycle_activity
    busy = @shot_lifecycle_activity.call.any? do |job|
      target = job["arguments"].is_a?(Hash) ? job["arguments"] : {}
      drama_id = job["dramaID"] || target["dramaID"]
      episode_id = job["episodeID"] || target["episodeID"]
      ACTIVE_SHOT_STATES.include?(job["state"]) && drama_id == arguments["dramaID"] &&
        (episode_id.to_s.empty? || episode_id == arguments["episodeID"])
    end
    raise DramaService::NotFound, "episode_busy" if busy
  end

  def remove_active_shot!(episode, arguments)
    shot = episode["shots"].find { |entry| entry["id"] == arguments["shotID"] }
    raise DramaService::NotFound, "shot_not_found" unless shot
    check_shot_membership_revision!(shot, arguments)
    episode["removedShots"] << { "shot" => deep_copy(shot), "originalOrder" => shot["order"],
                                 "removedAt" => Time.now.utc.iso8601 }
    episode["shots"].delete(shot)
  end

  def restore_archived_shot!(episode, arguments)
    archived = episode["removedShots"].find { |entry| entry.dig("shot", "id") == arguments["shotID"] }
    raise DramaService::NotFound, "removed_shot_not_found" unless archived
    shot = deep_copy(archived["shot"])
    check_shot_membership_revision!(shot, arguments)
    raise DramaService::NotFound, "shot_already_exists" if episode["shots"].any? { |entry| entry["id"] == shot["id"] }
    index = [[archived["originalOrder"].to_i - 1, 0].max, episode["shots"].length].min
    shot["rev"] = shot["rev"].to_i + 1
    episode["shots"].sort_by! { |entry| entry["order"].to_i }
    episode["shots"].insert(index, shot)
    episode["removedShots"].delete(archived)
  end

  def check_shot_membership_revision!(shot, arguments)
    check_revision(shot, arguments["expectedRev"])
    check_revision(shot, arguments["expectedDraftRev"], "draftRev")
  end

  def renumber_active_shots!(episode)
    episode["shots"].each_with_index do |shot, index|
      next if shot["order"] == index + 1
      shot["order"] = index + 1
      shot["rev"] = shot["rev"].to_i + 1
    end
  end
end

# frozen_string_literal: true

require_relative "production_film_audit"
require_relative "production_text_rules"
require_relative "production_meter"

class ProductionDelivery
  def initialize(film:, export_root:)
    @film, @root = film, export_root
  end

  def step(drama:, arguments:, permission:, update:)
    require!(permission["allSettled"] == true && permission.dig("usage", "unknown_requests") == [], "production_delivery_unsettled")
    if permission["conversationUsage"].is_a?(Hash) && !drama["production"].key?("conversationUsage")
      update.call { |current| current["production"]["conversationUsage"] = permission["conversationUsage"] }
      return { "pending" => true, "accountingSnapshotSaved" => true }
    end
    files = verified_films(drama)
    basis = ScreenplayRules.digest([files.map { |file| file.slice("episodeOrder", "sha256", "sourceDigest") }, permission["usage"], drama["production"]["seriesReviews"]["all"]])
    directory = directory_for(arguments["workflowID"])
    FileUtils.mkdir_p(directory)
    draft = drama["production"]["deliveryDraft"]
    draft = { "basis" => basis, "episodes" => [] } unless draft && draft["basis"] == basis
    if draft["episodes"].length < files.length
      artifact = files[draft["episodes"].length]
      name = "episode-#{format('%03d', artifact['episodeOrder'])}-#{artifact['sha256'][0, 16]}.mp4"
      path = File.join(directory, name)
      unless File.file?(path) && File.size(path) == artifact["bytes"] && Digest::SHA256.file(path).hexdigest == artifact["sha256"]
        copy_verified(artifact, path)
      end
      draft["episodes"] << artifact.slice("episodeOrder", "sha256", "bytes", "durationMs", "sourceDigest").merge("filename" => name)
      update.call { |current| current["production"]["deliveryDraft"] = draft }
      return { "pending" => true, "exportedEpisodes" => draft["episodes"].length, "totalEpisodes" => files.length }
    end
    require!(draft["episodes"].map { |file| file["episodeOrder"] } == (1..drama["episodeCount"]).to_a, "production_delivery_coverage_invalid")
    draft["episodes"].each do |artifact|
      path = File.join(directory, artifact["filename"])
      require!(File.file?(path) && File.size(path) == artifact["bytes"] && Digest::SHA256.file(path).hexdigest == artifact["sha256"], "production_delivery_file_changed")
    end
    records = JSON.pretty_generate(dossier(drama, permission["usage"])) + "\n"
    records_sha = Digest::SHA256.hexdigest(records)
    records_name = "records-#{records_sha[0, 24]}.json"
    records_path = File.join(directory, records_name)
    atomic_write(records_path, records) unless File.file?(records_path) && Digest::SHA256.file(records_path).hexdigest == records_sha
    records_file = { "filename" => records_name, "sha256" => records_sha, "bytes" => records.bytesize }
    manifest = { "schema" => "willdeep-production-delivery-v1", "workflowID" => arguments["workflowID"], "title" => drama["title"],
      "spec" => drama["production"]["spec"], "basis" => basis, "episodes" => draft["episodes"], "usage" => permission["usage"],
      "acceptance" => { "heardAudio" => true, "viewedFrames" => true, "decoded" => true, "episodeCount" => files.length,
        "boundaryCount" => files.length - 1, "seriesReviewDigest" => ScreenplayRules.digest(drama["production"]["seriesReviews"]["all"]) },
      "records" => records_file.merge("preserved" => true, "dramaID" => drama["id"], "sourceScreenplay" => drama["production"]["sourceFrozen"]&.slice("workflowID", "basis", "sha256"),
        "reviewDigest" => ScreenplayRules.digest(drama["production"].slice("issues", "audioAudits", "visualAudits", "videoAudits", "filmAudits", "boundaryAudits", "seriesReviews"))) }
    manifest["conversationUsage"] = drama["production"]["conversationUsage"] if drama["production"].key?("conversationUsage")
    bytes = JSON.pretty_generate(manifest) + "\n"
    sha = Digest::SHA256.hexdigest(bytes)
    name = "manifest-#{sha[0, 24]}.json"
    path = File.join(directory, name)
    atomic_write(path, bytes) unless File.file?(path) && Digest::SHA256.file(path).hexdigest == sha
    output = { "basis" => basis, "manifestSHA256" => sha, "manifestBytes" => bytes.bytesize, "manifestFilename" => name, "episodes" => draft["episodes"], "records" => records_file }
    update.call { |current| current["production"]["delivery"] = output }
    { "delivery" => output }
  end

  # Never accept a caller's path. Recheck Runtime completion, current domain
  # acceptance and the frozen export before returning any file descriptor.
  def read(drama:, workflow:, selector: nil)
    require!(workflow["id"] == drama.dig("production", "workflowID") && workflow["status"] == "completed" &&
      workflow.dig("usage", "unknown_requests") == [] && workflow.dig("usage", "reserved").to_h.values.all? { |amount| amount == 0 }, "production_delivery_not_completed")
    output = drama.dig("production", "delivery")
    require!(output && workflow.dig("domain", "delivery") == output, "production_delivery_runtime_mismatch")
    films = verified_films(drama)
    require!(output["episodes"].map { |file| file.slice("episodeOrder", "sha256", "bytes", "durationMs", "sourceDigest") } ==
      films.map { |file| file.slice("episodeOrder", "sha256", "bytes", "durationMs", "sourceDigest") }, "production_delivery_source_changed")
    directory = directory_for(workflow["id"])
    manifest = checked_file(directory, output["manifestFilename"], output["manifestSHA256"], output["manifestBytes"], "application/json")
    contents = JSON.parse(File.binread(manifest["filePath"]))
    require!(contents["workflowID"] == workflow["id"] && contents["basis"] == output["basis"] && contents["episodes"] == output["episodes"] &&
      contents["usage"] == workflow["usage"] && contents.dig("acceptance", "seriesReviewDigest") == ScreenplayRules.digest(drama["production"]["seriesReviews"]["all"]) &&
      contents.dig("records", "reviewDigest") == ScreenplayRules.digest(drama["production"].slice("issues", "audioAudits", "visualAudits", "videoAudits", "filmAudits", "boundaryAudits", "seriesReviews")), "production_delivery_manifest_stale")
    records = output.fetch("records")
    require!(contents["records"].slice("filename", "sha256", "bytes") == records, "production_delivery_records_mismatch")
    records_file = checked_file(directory, records["filename"], records["sha256"], records["bytes"], "application/json")
    require!(Digest::SHA256.hexdigest(JSON.pretty_generate(dossier(drama, workflow["usage"])) + "\n") == records["sha256"], "production_delivery_records_stale")
    if selector
      return manifest if selector == "manifest"
      return records_file if selector == "records"
      file = output["episodes"].find { |entry| "episode-#{entry['episodeOrder']}" == selector }
      require!(file, "production_delivery_selector_invalid")
      return checked_file(directory, file["filename"], file["sha256"], file["bytes"], "video/mp4").merge("episodeOrder" => file["episodeOrder"])
    end
    episodes = output["episodes"].map do |file|
      checked_file(directory, file["filename"], file["sha256"], file["bytes"], "video/mp4").reject { |key, _| key == "filePath" }
        .merge(file.slice("episodeOrder", "durationMs"), "selector" => "episode-#{file['episodeOrder']}")
    end
    { "manifest" => manifest.reject { |key, _| key == "filePath" }.merge("selector" => "manifest"),
      "records" => records_file.reject { |key, _| key == "filePath" }.merge("selector" => "records"), "episodes" => episodes }
  end

  def verified_films(drama)
    count = drama["production"]["spec"]["episode_count"]
    require!(drama["episodeCount"] == count && count.between?(1, 80), "production_delivery_count_changed")
    files = (1..count).map do |order|
      accepted = @film.accepted(drama, order)
      require!(accepted && accepted["heardAudio"] && accepted["viewedFrames"] && accepted["decoded"], "production_delivery_film_not_accepted")
      accepted.fetch("artifact")
    end
    basis = ScreenplayRules.digest(files.map { |artifact| artifact.slice("episodeOrder", "sha256", "sourceDigest") })
    acceptance = drama["production"].fetch("seriesReviews", {})["all"]
    require!(acceptance && acceptance["basis"] == basis && acceptance.dig("result", "accepted") == true && acceptance.dig("result", "coverage") == (1..count).to_a, "production_delivery_series_not_accepted")
    files.each_cons(2) do |left, right|
      key = ScreenplayRules.digest([left, right].map { |artifact| artifact.slice("episodeOrder", "sha256", "sourceDigest") })
      boundary = drama["production"].fetch("boundaryAudits", {})[key]
      require!(boundary && boundary.dig("result", "accepted") && boundary.dig("result", "heardAudio") && boundary.dig("result", "viewedFrames"), "production_delivery_boundary_not_accepted")
      file = boundary["file"]
      require!(file && File.file?(file["path"]) && File.size(file["path"]) == file["bytes"] && Digest::SHA256.file(file["path"]).hexdigest == file["sha256"], "production_delivery_boundary_file_changed")
    end
    files
  end

  private

  def directory_for(id)
    require!(id.is_a?(String) && ProductionMeter::UUID.match?(id), "production_delivery_id_invalid")
    File.join(File.expand_path(@root.respond_to?(:call) ? @root.call : @root), id)
  end

  def dossier(drama, usage)
    series = drama["production"]
    selected = series.slice("plan", "outlines", "outlineReviews", "episodes", "storyStates", "scriptReviews", "issues", "history", "storyboards", "assetCatalog", "audioAudits", "visualAudits", "videoAudits", "filmAudits", "boundaryAudits", "seriesReviews", "remoteRequests", "filmRepairs", "sourceFrozen")
    selected["mediaSteps"] = series.fetch("mediaSteps", {}).reject { |_key, step| step["stage"] == "deliver" }
    selected.merge!(series.slice("sourceCanon", "sourceCandidates", "sourceDerivative", "sourceSnapshotDigest", "conversationUsage"))
    clean({ "schema" => "willdeep-production-records-v1", "workflowID" => series["workflowID"], "title" => drama["title"], "spec" => series["spec"],
      "episodes" => drama.fetch("episodes", []), "characters" => drama.fetch("characters", []), "assets" => drama.fetch("assets", []), "records" => selected, "usage" => usage })
  end

  def clean(value)
    case value
    when Array then value.map { |entry| clean(entry) }
    when Hash
      value.reject { |key, _| key.match?(/(?:paths?|urls?)\z/i) || key.match?(/\A(?:api.?key|secret|authorization|credentials?|grant|headers|raw|response|data|base64)\z/i) }
        .transform_values { |entry| clean(entry) }
    when String
      value.match?(/\A(?:data:|https?:\/\/|file:)/i) ? "omitted-source-sha256:#{Digest::SHA256.hexdigest(value)}" : value
    else value
    end
  end

  def checked_file(directory, name, sha, bytes, mime)
    require!(name.is_a?(String) && name.match?(/\A(?:episode-\d{3}-[0-9a-f]{16}\.mp4|(?:manifest|records)-[0-9a-f]{24}\.json)\z/), "production_delivery_filename_invalid")
    path = File.join(directory, name)
    require!(!File.symlink?(path) && File.file?(path) && File.realpath(path).start_with?(File.realpath(directory) + File::SEPARATOR) &&
      File.size(path) == bytes && Digest::SHA256.file(path).hexdigest == sha, "production_delivery_file_changed")
    { "filename" => name, "sha256" => sha, "bytes" => bytes, "mimeType" => mime, "filePath" => path }
  end

  def require!(condition, code)
    ProductionTextRules.require!(condition, code)
  end

  def copy_verified(artifact, destination)
    temporary = "#{destination}.#{SecureRandom.hex(6)}.tmp"
    File.open(artifact["filePath"], "rb") { |source| File.open(temporary, "wb", 0o600) { |target| IO.copy_stream(source, target) } }
    require!(File.size(temporary) == artifact["bytes"] && Digest::SHA256.file(temporary).hexdigest == artifact["sha256"], "production_delivery_copy_changed")
    File.rename(temporary, destination)
  ensure
    File.unlink(temporary) if temporary && File.file?(temporary)
  end

  def atomic_write(path, bytes)
    temporary = "#{path}.#{SecureRandom.hex(6)}.tmp"
    File.open(temporary, "wb", 0o600) { |file| file.write(bytes); file.flush; file.fsync }
    File.rename(temporary, path)
  ensure
    File.unlink(temporary) if temporary && File.file?(temporary)
  end
end

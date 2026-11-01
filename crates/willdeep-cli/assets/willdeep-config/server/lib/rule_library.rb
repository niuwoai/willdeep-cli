# 规则库：列出、查看、编辑 ~/.willdeep/rules 里的规则，以及从 GitHub 安装一份。
#
# 一条规则是一个文件，或一个目录（目录里的 *.md 按文件名拼接）。哪些规则生效由
# 配置里的 [rules].files 决定，这个库只负责文件本身；启用与排序由页面写回配置。
module RuleLibrary
  MAX_CONTENT_BYTES = 256 * 1024
  BUNDLED_NAME = "agentrules.md"

  module_function

  def list(home, configured)
    directory = rules_dir(home)
    files = Dir.exist?(directory) ? Dir.children(directory).sort : []
    entries = files.map do |name|
      path = File.join(directory, name)
      next unless File.file?(path) || File.directory?(path)
      {
        "name" => name,
        "path" => path,
        "kind" => File.directory?(path) ? "directory" : "file",
        "bytes" => File.directory?(path) ? nil : File.size(path),
        "enabled" => configured.nil? ? false : configured.any? { |item| same_path?(home, item, path) }
      }
    end.compact
    {
      "ok" => true,
      "rulesDir" => directory,
      "configured" => configured,
      "entries" => entries,
      "bundledAvailable" => bundled_path && File.file?(bundled_path)
    }
  end

  def read(home, path)
    file = editable_file(home, path)
    return file if file["ok"] == false
    return error("is_directory", "这是一个目录规则，打开里面的单个文件编辑") if File.directory?(file)
    { "ok" => true, "content" => File.read(file, encoding: "utf-8"), "path" => file }
  end

  def write(home, path, content)
    return error("invalid_edits", "content 必须是字符串") unless content.is_a?(String)
    return error("invalid_edits", "规则正文超过 256 KiB") if content.bytesize > MAX_CONTENT_BYTES
    file = editable_file(home, path)
    return file if file["ok"] == false
    return error("is_directory", "不能把一个目录规则写成文件") if File.directory?(file)
    File.write(file, content)
    { "ok" => true, "path" => file }
  end

  # 从 GitHub 安装一份规则，存成规则目录里的一个文件。仓库根上的 README 与说明性
  # 文档不收，只收规则正文。同名已存在时不覆盖。
  def install(home, source, name)
    parsed = SkillLibrary.parse_source(source)
    return error("invalid_edits", parsed[:error]) if parsed[:error]
    name = name.to_s.strip
    name = "#{File.basename(parsed[:url].split("/")[4])}.md" if name.empty?
    name = "#{name}.md" unless name.end_with?(".md")
    return error("invalid_edits", "规则文件名不合法") unless name =~ /\A[A-Za-z0-9_.-]{1,64}\z/
    destination = File.join(rules_dir(home), name)
    return error("exists", "已有同名规则 #{name}，没有覆盖") if File.exist?(destination)
    staging = Dir.mktmpdir("willdeep-rule-")
    begin
      archive = File.join(staging, "repo.zip")
      extracted = File.join(staging, "src")
      SkillLibrary.download(parsed[:url], archive)
      SkillLibrary.extract(archive, extracted)
      content = collect(extracted, parsed[:subdir])
      return error("not_found", "仓库里没有找到规则文件") if content.nil? || content.strip.empty?
      return error("invalid_edits", "规则正文超过 256 KiB") if content.bytesize > MAX_CONTENT_BYTES
      FileUtils.mkdir_p(rules_dir(home))
      File.write(destination, content)
      { "ok" => true, "path" => destination }
    ensure
      FileUtils.rm_rf(staging)
    end
  rescue SkillLibrary::InstallError => e
    error("install_failed", e.message)
  end

  # 把随程序发布的精简规则写进规则目录。目录里已有同名文件就不覆盖。
  def restore_bundled(home)
    source = bundled_path
    return error("not_found", "没有内置规则") unless source && File.file?(source)
    destination = File.join(rules_dir(home), BUNDLED_NAME)
    return error("exists", "已有 #{BUNDLED_NAME}，没有覆盖") if File.exist?(destination)
    FileUtils.mkdir_p(rules_dir(home))
    FileUtils.cp(source, destination)
    { "ok" => true, "path" => destination }
  end

  def rules_dir(home)
    File.join(home, "rules")
  end

  # 内置规则随配置插件一起发布，放在资源目录的 rules/ 下。
  def bundled_path
    candidate = File.expand_path("../../rules/#{BUNDLED_NAME}", __dir__)
    File.file?(candidate) ? candidate : nil
  end

  def editable_file(home, path)
    text = path.to_s.strip
    return error("invalid_edits", "路径不能为空") if text.empty?
    return error("invalid_edits", "路径不能包含 ..") if text.split(%r{[/\\]}).any? { |part| part == ".." }
    expanded = SkillLibrary.expand(text)
    root = File.expand_path(rules_dir(home))
    unless expanded == root || expanded.start_with?(root + File::SEPARATOR)
      return error("forbidden", "只能编辑 #{root} 里的规则")
    end
    return error("not_found", "#{expanded} 不存在") unless File.exist?(expanded)
    expanded
  end

  def same_path?(home, configured, path)
    File.expand_path(SkillLibrary.expand(configured.to_s)) == File.expand_path(path)
  rescue ArgumentError
    false
  end

  # 把一个仓库里的规则 Markdown 收成一份文本。README 与仓库说明文档不算规则。
  def collect(extracted, subdir)
    top = Dir.children(extracted).map { |name| File.join(extracted, name) }.select { |path| File.directory?(path) }
    return nil unless top.length == 1
    base = subdir.to_s.empty? ? top[0] : File.join(top[0], subdir)
    return nil unless File.directory?(base)
    files = Dir.children(base).sort.select do |name|
      name =~ /\.md\z/i && !%w[readme.md changelog.md product_overview.md architecture.md].include?(name.downcase)
    end
    return nil if files.empty?
    files.map { |name| File.read(File.join(base, name), encoding: "utf-8") }.join("\n\n")
  end

  def error(code, message)
    { "ok" => false, "error" => code, "message" => message }
  end
end

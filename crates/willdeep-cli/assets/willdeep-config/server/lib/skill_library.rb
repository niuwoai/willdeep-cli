# 技能库：列出、查看、编辑、安装 ~/.willdeep/skills 里的技能。
#
# 一个技能是一个目录加一份 SKILL.md。发现范围与 willdeep-rs 的 SkillCatalog 一致：
# 用户主目录下的三个固定目录，加上配置里 [skills].roots 的额外目录。工作区里的技能
# 不在这里列——配置页没有「当前工作区」这个概念，那些技能跟着项目走。
#
# 只有用户自己目录里的技能可以改或删。额外目录里的技能只读，避免这个页面改到
# 一份别人维护的共享技能库。
module SkillLibrary
  SKILL_FILE = "SKILL.md"
  MAX_CONTENT_BYTES = 256 * 1024
  MAX_DOWNLOAD_BYTES = 32 * 1024 * 1024
  NAME_PATTERN = /\A[A-Za-z0-9_-]{1,64}\z/

  module_function

  def list(home, extra_roots)
    roots = fixed_roots(home) + Array(extra_roots)
    skills = []
    seen = {}
    roots.each do |root|
      expanded = expand(root)
      next unless File.directory?(expanded)
      Dir.children(expanded).sort.each do |name|
        directory = File.join(expanded, name)
        file = File.join(directory, SKILL_FILE)
        next unless File.file?(file)
        identifier = normalize(name)
        next if identifier.empty? || seen[identifier]
        seen[identifier] = true
        meta = metadata(File.read(file, encoding: "utf-8"), name)
        skills << {
          "identifier" => identifier,
          "name" => meta[:name],
          "description" => meta[:description],
          "tier" => meta[:tier],
          "directory" => directory,
          "editable" => inside_user_dir?(home, directory)
        }
        break if skills.length >= 300
      end
    end
    { "ok" => true, "skills" => skills, "userDir" => user_dir(home) }
  end

  def read(directory)
    file = skill_file(directory)
    return error("not_found", "#{directory} 里没有 #{SKILL_FILE}") unless file
    { "ok" => true, "content" => File.read(file, encoding: "utf-8"), "path" => file }
  end

  def write(home, directory, content)
    return error("invalid_edits", "content 必须是字符串") unless content.is_a?(String)
    return error("invalid_edits", "技能正文超过 256 KiB") if content.bytesize > MAX_CONTENT_BYTES
    file = skill_file(directory)
    return error("not_found", "#{directory} 里没有 #{SKILL_FILE}") unless file
    return error("forbidden", "只能修改 #{user_dir(home)} 里的技能") unless inside_user_dir?(home, file)
    File.write(file, content)
    { "ok" => true, "path" => file }
  end

  def create(home, name, content)
    name = name.to_s.strip
    return error("invalid_edits", "技能名只能用字母、数字、连字符，最长 64 个字符") unless name =~ NAME_PATTERN
    return error("invalid_edits", "content 必须是字符串") unless content.is_a?(String)
    directory = File.join(user_dir(home), name)
    return error("exists", "已有同名技能：#{name}") if File.exist?(directory)
    FileUtils.mkdir_p(directory)
    File.write(File.join(directory, SKILL_FILE), content)
    { "ok" => true, "directory" => directory }
  rescue SystemCallError => e
    FileUtils.rm_rf(directory) if directory && File.directory?(directory)
    error("write_failed", e.message)
  end

  def remove(home, directory)
    file = skill_file(directory)
    return error("not_found", "#{directory} 不是技能目录") unless file
    return error("forbidden", "只能删除 #{user_dir(home)} 里的技能") unless inside_user_dir?(home, file)
    FileUtils.rm_rf(File.dirname(file))
    { "ok" => true }
  end

  # 从 GitHub 地址安装。地址形如 owner/repo，或带分支与子目录的
  # https://github.com/owner/repo/tree/<分支>/<路径>。
  def install(home, source)
    parsed = parse_source(source)
    return error("invalid_edits", parsed[:error]) if parsed[:error]
    staging = Dir.mktmpdir("willdeep-skill-")
    begin
      archive = File.join(staging, "repo.zip")
      download(parsed[:url], archive)
      extracted = File.join(staging, "src")
      extract(archive, extracted)
      skills = find_skills(extracted, parsed[:subdir])
      return error("not_found", "仓库里没有找到 SKILL.md") if skills.empty?
      installed = skills.map { |directory| copy_skill(home, directory) }
      { "ok" => true, "installed" => installed }
    ensure
      FileUtils.rm_rf(staging)
    end
  rescue InstallError => e
    error("install_failed", e.message)
  end

  def user_dir(home)
    File.join(home, "skills")
  end

  def fixed_roots(home)
    [File.join(home, "skills"), File.join(home, ".agents", "skills"), File.join(home, ".codex", "skills")]
  end

  def inside_user_dir?(home, path)
    root = File.expand_path(user_dir(home))
    expanded = File.expand_path(path)
    expanded == root || expanded.start_with?(root + File::SEPARATOR)
  end

  def skill_file(directory)
    return nil if directory.to_s.strip.empty?
    file = File.join(File.expand_path(directory), SKILL_FILE)
    File.file?(file) ? file : nil
  end

  def expand(path)
    value = path.to_s
    if value.start_with?("~") && (value.length == 1 || value[1] == "/")
      value = File.join(Dir.home, value[1..-1].to_s.sub(%r{\A/}, ""))
    end
    File.expand_path(value)
  end

  def normalize(name)
    name.chars.map { |c| c =~ /[A-Za-z0-9_-]/ ? c.downcase : "-" }.join.gsub(/\A-+|-+\z/, "")
  end

  # 只认 frontmatter 里的 name / description / tier 三行，与 Rust 侧同一口径。
  def metadata(body, fallback)
    name = nil
    description = nil
    tier = nil
    if body.start_with?("---")
      body.lines.drop(1).take_while { |line| line.strip != "---" }.each do |line|
        name = unquote($1) if line =~ /\Aname:\s*(.*)\s*\z/
        description = unquote($1) if line =~ /\Adescription:\s*(.*)\s*\z/
        tier = unquote($1) if line =~ /\Atier:\s*(.*)\s*\z/
      end
    end
    tier = nil unless %w[worker small standard medium deep large].include?(tier.to_s.downcase)
    {
      name: name.to_s.empty? ? fallback : name,
      description: description.to_s.empty? ? "Installed skill" : description,
      tier: tier
    }
  end

  def unquote(value)
    value.to_s.strip.gsub(/\A['"]|['"]\z/, "")
  end

  def parse_source(input)
    text = input.to_s.strip.sub(%r{/\z}, "")
    return { error: "地址不能为空" } if text.empty?
    if text.include?("://")
      scheme, rest = text.split("://", 2)
      return { error: "只接受 https 的 GitHub 地址" } unless scheme.downcase == "https"
      host, path = rest.split("/", 2)
      return { error: "地址里没有仓库路径" } if path.nil?
    else
      host = "github.com"
      path = text
    end
    return { error: "只能安装 github.com 上的仓库，得到 #{host}" } unless %w[github.com www.github.com].include?(host)
    parts = path.split("/").reject(&:empty?)
    return { error: "地址需要拥有者和仓库名" } if parts.length < 2
    owner = parts[0]
    repo = parts[1].sub(/\.git\z/, "")
    return { error: "仓库名不合法" } unless owner =~ NAME_PATTERN && repo =~ NAME_PATTERN
    reference = "main"
    subdir = ""
    if parts.length > 2
      return { error: "不认识的地址形式，用 owner/repo 或 /tree/<分支>/<路径>" } unless %w[tree blob].include?(parts[2])
      return { error: "地址没有分支名" } if parts[3].to_s.empty?
      reference = parts[3]
      subdir = parts[4..-1].to_a.join("/")
      return { error: "子目录不合法" } if subdir.split("/").any? { |part| part.empty? || part == ".." }
    end
    { url: "https://codeload.github.com/#{owner}/#{repo}/zip/refs/heads/#{reference}", subdir: subdir }
  end

  class InstallError < StandardError; end

  def download(url, destination)
    unless system("curl", "--silent", "--show-error", "--fail", "--location", "--max-time", "60",
                  "--output", destination, url, out: File::NULL, err: File::NULL)
      raise InstallError, "下载失败。仓库必须是公开的，分支必须存在。"
    end
    size = File.size(destination)
    raise InstallError, "下载到的文件大小异常（#{size} 字节）" if size <= 0 || size > MAX_DOWNLOAD_BYTES
  end

  def extract(archive, destination)
    names = zip_names(archive)
    names.each do |name|
      raise InstallError, "压缩包条目 #{name} 路径不合法" unless safe_entry?(name)
    end
    FileUtils.mkdir_p(destination)
    ok = if RUBY_PLATFORM =~ /darwin/
           system("ditto", "-xk", archive, destination, out: File::NULL, err: File::NULL)
         else
           system("unzip", "-q", "-o", archive, "-d", destination, out: File::NULL, err: File::NULL)
         end
    raise InstallError, "解压失败" unless ok
  end

  def zip_names(archive)
    command = RUBY_PLATFORM =~ /darwin/ ? ["zipinfo", "-1", archive] : ["unzip", "-Z1", archive]
    output = IO.popen(command, "r", &:read)
    raise InstallError, "无法读取压缩包内容" unless $?.success?
    output.to_s.lines.map(&:strip).reject(&:empty?)
  end

  def safe_entry?(name)
    return false if name.empty? || name.start_with?("/") || name.include?("\\")
    !name.split("/").any? { |part| part == ".." }
  end

  def find_skills(extracted, subdir)
    top = Dir.children(extracted).map { |name| File.join(extracted, name) }.select { |path| File.directory?(path) }
    return [] unless top.length == 1
    base = subdir.empty? ? top[0] : File.join(top[0], subdir)
    return [] unless File.directory?(base)
    return [base] if File.file?(File.join(base, SKILL_FILE))
    Dir.children(base).sort.map { |name| File.join(base, name) }
       .select { |path| File.directory?(path) && File.file?(File.join(path, SKILL_FILE)) }
  end

  def copy_skill(home, source)
    name = File.basename(source)
    raise InstallError, "技能目录名不合法：#{name}" unless name =~ NAME_PATTERN
    destination = File.join(user_dir(home), name)
    raise InstallError, "已安装同名技能 #{name}，没有覆盖" if File.exist?(destination)
    FileUtils.mkdir_p(destination)
    Dir.each_child(source) do |child|
      next if File.symlink?(File.join(source, child))
      FileUtils.cp_r(File.join(source, child), destination)
    end
    name
  end

  def error(code, message)
    { "ok" => false, "error" => code, "message" => message }
  end
end

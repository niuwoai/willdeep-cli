# frozen_string_literal: true

require "digest"
require "fileutils"
require "json"
require "open3"
require "optparse"
require "rbconfig"
require "tmpdir"

DESKTOP = File.expand_path(__dir__)
ROOT = File.expand_path("..", DESKTOP)
BUILD = File.join(ROOT, ".build", "desktop")
APP = "WillDeep Desktop"
TAURI_CLI = File.join(DESKTOP, "node_modules", "@tauri-apps", "cli", "tauri.js")
ARM_MAC = "aarch64-apple-darwin"
INTEL_MAC = "x86_64-apple-darwin"
UNIVERSAL_MAC = "universal-apple-darwin"

def run!(*command, directory: ROOT, env: {})
  return if system(env, *command, chdir: directory)

  abort "Build command failed: #{command.take(2).join(' ')}"
end

def capture!(*command)
  output, status = Open3.capture2(*command, chdir: ROOT)
  abort "Build command failed: #{command.take(2).join(' ')}" unless status.success?

  output
end

def stage_runtime(target, host, cargo_target)
  extension = target.include?("windows") ? ".exe" : ""
  build_args = target == host ? [] : ["--target", target]
  run!("cargo", "build", "--release", "--locked", "-p", "willdeep", *build_args)
  output_target = target == host ? cargo_target : File.join(cargo_target, target)
  source = File.join(output_target, "release", "willdeep#{extension}")
  binaries = File.join(DESKTOP, "binaries")
  destination = File.join(binaries, "willdeep-#{target}#{extension}")
  FileUtils.mkdir_p(binaries)
  Dir.mktmpdir("stage-", binaries) do |staging|
    staged = File.join(staging, File.basename(destination))
    FileUtils.cp(source, staged)
    # macOS can keep the old code signature cached when replacing executable bytes in place.
    FileUtils.mv(staged, destination, force: true)
  end
  destination
end

def make_mac_image(app, version, target)
  runtime = File.join(app, "Contents", "MacOS", "willdeep")
  if target == UNIVERSAL_MAC
    main = File.join(app, "Contents", "MacOS", "willdeep-desktop")
    [main, runtime].each do |binary|
      architectures = capture!("lipo", "-archs", binary).split
      abort "Missing macOS architecture in #{binary}" unless (%w[arm64 x86_64] - architectures).empty?
    end
  end
  run!("codesign", "--force", "--sign", "-", "--timestamp=none", runtime)
  run!("codesign", "--force", "--sign", "-", "--timestamp=none", app)
  run!("codesign", "--verify", "--deep", "--strict", app)
  arch = target == UNIVERSAL_MAC ? "universal" : target.split("-").first
  image = File.join(BUILD, "WillDeep-Desktop-#{version}-macOS-#{arch}.dmg")
  Dir.mktmpdir("willdeep-desktop-", BUILD) do |staging|
    run!("ditto", app, File.join(staging, "#{APP}.app"))
    File.symlink("/Applications", File.join(staging, "Applications"))
    run!("hdiutil", "create", "-ov", "-volname", APP, "-format", "UDZO", "-srcfolder", staging, image)
  end
  run!("hdiutil", "verify", image)
  puts "#{image}\nSHA-256 #{Digest::SHA256.file(image).hexdigest}"
end

host = capture!("rustc", "-vV").lines.grep(/^host: /).first.split.last
mac = RbConfig::CONFIG.fetch("host_os").include?("darwin")
target = mac ? UNIVERSAL_MAC : host
OptionParser.new { |parser| parser.on("--target TARGET") { |value| target = value } }.parse!
abort "Unexpected arguments: #{ARGV.join(' ')}" unless ARGV.empty?

version = JSON.parse(File.read(File.join(DESKTOP, "tauri.conf.json"))).fetch("version")
run!(RbConfig.ruby, "scripts/check_release_version.rb", version)
run!("yarn", "--cwd", "web", "build")
run!("yarn", "tauri", "icon", "../web/src/assets/willdeep-app-icon.png", directory: DESKTOP)
cargo_target = JSON.parse(capture!("cargo", "metadata", "--locked", "--no-deps", "--format-version", "1")).fetch("target_directory")
targets = target == UNIVERSAL_MAC ? [ARM_MAC, INTEL_MAC] : [target]
runtime_binaries = targets.map { |triple| stage_runtime(triple, host, cargo_target) }
if target == UNIVERSAL_MAC
  binaries = File.join(DESKTOP, "binaries")
  Dir.mktmpdir("universal-", binaries) do |staging|
    name = "willdeep-#{UNIVERSAL_MAC}"
    universal_runtime = File.join(staging, name)
    run!("lipo", "-create", *runtime_binaries, "-output", universal_runtime)
    FileUtils.chmod(0o755, universal_runtime)
    FileUtils.mv(universal_runtime, File.join(binaries, name), force: true)
  end
end

FileUtils.mkdir_p(BUILD)
desktop_target = File.join(DESKTOP, "target")
bundle = if mac
           "app"
         elsif host.include?("windows")
           "nsis"
         else
           "deb"
         end
signing = mac ? ["--no-sign"] : []
run!("node", TAURI_CLI, "build", "--ci", "--target", target, "--bundles", bundle, *signing, "--", "--locked",
     directory: DESKTOP, env: { "CARGO_TARGET_DIR" => desktop_target })

bundles = File.join(desktop_target, target, "release", "bundle")
if mac
  make_mac_image(File.join(bundles, "macos", "#{APP}.app"), version, target)
else
  puts File.join(bundles, bundle)
end

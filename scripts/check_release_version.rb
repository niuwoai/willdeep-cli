#!/usr/bin/env ruby
# frozen_string_literal: true

require 'json'

version = ARGV.fetch(0)
abort "invalid release version: #{version.inspect}" unless version.match?(/\A\d+\.\d+\.\d+-rc\d+\z/)
root = File.expand_path('..', __dir__)
read = ->(path) { File.read(File.join(root, path), encoding: 'UTF-8') }
check = lambda do |path, actual, expected = version|
  abort "#{path}: expected #{expected.inspect}, found #{actual.inspect}" unless actual == expected
end

cargo = read.call('Cargo.toml')
check.call('Cargo.toml workspace', cargo[/^version = "([^"]+)"$/, 1])
%w[willdeep-runtime-protocol willdeep-runtime-client].each do |name|
  check.call("Cargo.toml #{name}", cargo[/^#{Regexp.escape(name)} = \{[^\n]*version = "([^"]+)"/, 1])
end
lock = read.call('Cargo.lock')
%w[willdeep willdeep-core willdeep-runtime-client willdeep-runtime-protocol].each do |name|
  check.call("Cargo.lock #{name}", lock[/^name = "#{Regexp.escape(name)}"\nversion = "([^"]+)"$/, 1])
end
%w[web/package.json npm/willdeep/package.json npm/willdeep/install-manifest.json].each do |path|
  check.call(path, JSON.parse(read.call(path)).fetch('version'))
end
manifest = JSON.parse(read.call('npm/willdeep/install-manifest.json'))
check.call('install manifest tag', manifest.fetch('tag'), "v#{version}")
%w[PRODUCT_OVERVIEW.md docs/ARCHITECTURE.md].each do |path|
  check.call(path, read.call(path).lines.fetch(2)[/版本：v([^（\s]+)/, 1])
end
abort "CHANGELOG.md: missing #{version} entry" unless read.call('CHANGELOG.md').include?("## [#{version}]")
puts "Release version metadata matches #{version}"

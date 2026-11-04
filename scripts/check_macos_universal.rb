#!/usr/bin/env ruby
# frozen_string_literal: true

require 'open3'

UNIVERSAL_ARCHITECTURES = %w[arm64 x86_64].freeze
abort 'usage: check_macos_universal.rb BINARY' unless ARGV.length == 1
binary = ARGV.fetch(0)
abort "not an ordinary binary: #{binary}" unless File.file?(binary) && !File.symlink?(binary)
output, error, status = Open3.capture3('lipo', '-archs', binary)
abort "inspect #{binary}: #{error.strip}" unless status.success?
architectures = output.split.sort
unless architectures == UNIVERSAL_ARCHITECTURES
  abort "#{binary}: expected #{UNIVERSAL_ARCHITECTURES.join(' ')}, found #{architectures.join(' ')}"
end
puts "Verified universal architectures: #{architectures.join(' ')}"

# frozen_string_literal: true

require 'open3'
require 'tmpdir'

# Final-code contract only: this is not an LSP or edit-tool protocol verifier.
root = File.expand_path('..', __dir__)
pricing = File.read(File.join(root, 'src/pricing.rs'), encoding: 'UTF-8')
abort 'old pricing::quote API must be removed' if pricing.match?(/\bpub\s+fn\s+quote\s*\(/)

Dir.mktmpdir('p0-rust-reference-') do |output|
  library = File.join(output, 'libreference_shop.rlib')
  binary = File.join(output, 'contract')
  commands = [
    ['rustc', '--edition=2021', '--crate-name', 'reference_shop', '--crate-type=lib',
     File.join(root, 'src/lib.rs'), '-o', library],
    ['rustc', '--edition=2021', '--test', File.join(__dir__, 'contract.rs'),
     '--extern', "reference_shop=#{library}", '-o', binary],
    [binary, '--test-threads=1']
  ]
  commands.each do |command|
    stdout, stderr, status = Open3.capture3(*command, chdir: root)
    next if status.success?

    warn stdout
    warn stderr
    abort "Rust final-code contract failed: #{command.first} (#{status.exitstatus})"
  end
end

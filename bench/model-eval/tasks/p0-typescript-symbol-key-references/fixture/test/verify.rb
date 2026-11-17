# frozen_string_literal: true

require 'open3'

# Fixed-fixture structural checks plus native Node execution, not a TS compiler.
root = File.expand_path('..', __dir__)
version, version_error, version_status = Open3.capture3('node', '--version')
abort "cannot query Node version: #{version_error}" unless version_status.success?
minimum = [22, 18, 0]
actual = version.sub(/\Av/, '').strip.split('.').map(&:to_i)
abort 'Node >=22.18.0 is required for native TypeScript execution; no install is attempted' if (actual <=> minimum).negative?

formatter = File.read(File.join(root, 'src/formatter.ts'), encoding: 'UTF-8')
view = File.read(File.join(root, 'src/view.ts'), encoding: 'UTF-8')
# These patterns deliberately cover this fixture's declared reference forms only.
# Runtime tests independently exercise the code so comments cannot satisfy them alone.
checks = {
  'Formatter type declares the new property' => formatter.match?(/interface\s+Formatter\s*\{\s*renderLabel\s*\(/),
  'old target declarations are removed' => !formatter.match?(/\bformatLabel\s*(?:\(|:)/),
  'dot reference migrates' => view.match?(/formatter\s*\.\s*renderLabel\s*\(/),
  'string-key reference migrates' => view.match?(/formatter\s*\[\s*["']renderLabel["']\s*\]\s*\(/),
  'destructuring reference migrates' => view.match?(/\{\s*renderLabel\s*:\s*render\s*\}\s*=\s*formatter/),
  'old dot and string-key references are removed' => !view.match?(/formatter\s*(?:\.\s*formatLabel|\[\s*["']formatLabel["']\s*\])/)
}
checks.each { |label, ok| abort "TypeScript reference contract failed: #{label}" unless ok }

stdout, stderr, status = Open3.capture3('node', '--experimental-strip-types', '--test', File.join(__dir__, 'references.test.ts'), chdir: root)
unless status.success?
  warn stdout
  warn stderr
  abort "TypeScript runtime contract failed (#{status.exitstatus})"
end

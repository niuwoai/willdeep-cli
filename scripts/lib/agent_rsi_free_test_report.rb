# frozen_string_literal: true
require 'fileutils'
require 'json'

module AgentRsiFreeTestReport
  class << self
    attr_accessor :reporter
    def minitest_plugin_init(_options) = self.reporter = Minitest.reporter

    def enable(stem)
      Minitest.extensions << self
      index = ARGV.index('--report-directory')
      return unless index
      directory = File.expand_path(ARGV.fetch(index + 1))
      ARGV.slice!(index, 2)
      Minitest.after_run do
        summary = reporter.reporters.find { |r| r.is_a?(Minitest::SummaryReporter) }
        data = {'tests' => summary.count, 'assertions' => summary.assertions, 'failures' => summary.failures,
          'errors' => summary.errors, 'skipped' => summary.skips, 'model_requests' => 0,
          'passed' => summary.failures.zero? && summary.errors.zero?}
        FileUtils.mkdir_p(directory)
        File.write(File.join(directory, stem + '.json'), JSON.pretty_generate(data) + "\n")
        File.write(File.join(directory, stem + '.md'), "# 免费回归：#{stem}\n\n#{data['tests']} 项，#{data['assertions']} 个断言；失败 #{data['failures']}，错误 #{data['errors']}。新增模型请求：0。\n")
      end
    end
  end
end

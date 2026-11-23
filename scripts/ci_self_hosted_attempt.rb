#!/usr/bin/env ruby
# frozen_string_literal: true

# A hosted coordinator owns the deadline; a queued self-hosted job cannot own it.
require 'json'
require 'net/http'
require 'open3'
require 'securerandom'
require 'timeout'
require 'uri'

module CiSelfHostedAttempt
  WORKFLOW = '.github/workflows/ci-self-hosted.yml'
  QUEUE_SECONDS = 300
  EXECUTION_SECONDS = 3600
  # Two platforms, three reads per poll: 720 requests/hour, plus dispatch/cleanup.
  POLL_SECONDS = 30
  CLEANUP_SECONDS = 45
  PARENT_CLEANUP_SECONDS = 5
  PLATFORMS = {
    'macos-latest' => %w[self-hosted macOS X64 imac],
    'ubuntu-latest' => %w[self-hosted Linux X64 q4-pub]
  }.freeze
  REQUIRED_STEPS = ['Set up job', 'Validate worker identity', 'Checkout expected revision',
                    'Run complete CI suite', 'Self-hosted suite passed'].freeze
  Result = Struct.new(:passed, :reason, :child_id, :cleanup, keyword_init: true)
  Response = Struct.new(:status, :data, keyword_init: true)
  class Error < StandardError; end
  class ParentCancelled < Error; end
  class CleanupTimeout < Error; end

  class ApiError < Error
    attr_reader :status

    def initialize(method, path, status)
      @status = status
      super("GitHub API #{method} #{path.split('?').first}: HTTP #{status}")
    end
  end

  class Api
    def initialize(token:, origin: 'https://api.github.com')
      @token = token
      @origin = URI(origin)
      unless @origin.scheme == 'https' && @origin.host && !@origin.userinfo && !@origin.query && !@origin.fragment
        raise Error, 'invalid GitHub API origin'
      end
    end

    def request(method, path, body = nil)
      uri = URI("#{@origin.to_s.sub(%r{/$}, '')}#{path}")
      request = (method == :get ? Net::HTTP::Get : Net::HTTP::Post).new(uri)
      request['Authorization'] = "Bearer #{@token}"
      request['Accept'] = 'application/vnd.github+json'
      request['X-GitHub-Api-Version'] = '2026-03-10'
      request['User-Agent'] = 'willdeep-ci-runner-coordinator'
      if body
        request['Content-Type'] = 'application/json'
        request.body = JSON.generate(body)
      end
      http = Net::HTTP.new(uri.host, uri.port)
      http.use_ssl = true
      http.open_timeout = 5
      http.read_timeout = 10
      http.write_timeout = 10
      # A single operation is bounded even when the remote keeps dribbling bytes.
      response = Timeout.timeout(25) { http.start { |connection| connection.request(request) } }
      status = response.code.to_i
      raise ApiError.new(method, path, status) unless (200..299).cover?(status)

      data = response.body.to_s.empty? ? {} : JSON.parse(response.body)
      Response.new(status: status, data: data)
    rescue JSON::ParserError
      raise Error, 'GitHub API returned invalid JSON'
    rescue Timeout::Error, IOError, SystemCallError, SocketError, OpenSSL::SSL::SSLError => error
      # Never print token, response body or exception text from the transport.
      raise Error, "GitHub API transport failed (#{error.class})"
    end
  end

  class Coordinator
    def initialize(api:, repo:, ref:, sha:, os:, parent_id:, parent_attempt:, nonce: SecureRandom.hex(16),
                   dispatch_ref: ref, dispatch_sha: sha,
                   clock: -> { Process.clock_gettime(Process::CLOCK_MONOTONIC) }, sleeper: ->(seconds) { sleep seconds })
      @api, @repo, @ref, @sha, @os = api, repo, ref, sha, os
      @dispatch_ref, @dispatch_sha = dispatch_ref, dispatch_sha
      @parent_id, @parent_attempt = parent_id.to_s, parent_attempt.to_s
      @request_id = "#{@parent_id}-#{@parent_attempt}-#{@os}-#{nonce}"
      @clock, @sleeper = clock, sleeper
      @cleanup_note = 'not needed'
    end

    def attempt
      passed = false
      reason = 'self-hosted attempt did not finish'
      begin
        validate!
        @queue_start = now
        # Set before POST: a lost response may still have created our child.
        @dispatched = true
        response = @api.request(:post, "#{workflow_base}/dispatches", {
          'ref' => branch, 'inputs' => { 'worker_os' => @os, 'expected_sha' => @sha,
                                       'workflow_sha' => @dispatch_sha, 'request_id' => @request_id }
        })
        unless [200, 204].include?(response.status)
          raise Error, 'unexpected workflow dispatch response'
        end
        if response.status == 200
          id = response.data['workflow_run_id']
          raise Error, 'dispatch response has no valid workflow_run_id' unless positive_id?(id)

          @child_id = id.to_i
        end
        passed, reason = watch
      rescue ParentCancelled
        @parent_cancelled = true
        reason = 'parent cancelled; cleaning up its own self-hosted attempt'
      rescue ApiError => error
        reason = if [404, 422].include?(error.status) && !@child_id
                   "#{error.message}; bootstrap: workflow_dispatch must first exist on the default branch; hosted fallback until merged"
                 else
                   "#{error.message}; using hosted fallback"
                 end
      rescue StandardError => error
        reason = error.is_a?(Error) ? error.message : "coordinator error (#{error.class}); using hosted fallback"
      ensure
        cleanup unless passed || !@dispatched
      end
      Result.new(passed: passed, reason: reason, child_id: @child_id, cleanup: @cleanup_note)
    end

    # The first signal interrupts polling; subsequent INT/TERM must not interrupt
    # the bounded cleanup while GitHub's cancellation escalation clock is running.
    def parent_cancel!
      return if @parent_cancelled

      @parent_cancelled = true
      raise ParentCancelled, 'parent cancelled by signal'
    end

    private

    def validate!
      unless @repo.match?(%r{\A[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9_.-]+\z}) && !%w[. ..].include?(@repo.split('/').last)
        raise Error, 'invalid repository'
      end
      # PR CI tests a synthetic merge commit, but workflow_dispatch needs its real
      # source branch. Keep the tested revision separate from the workflow identity.
      pull_request = @ref.match?(%r{\Arefs/pull/[1-9][0-9]*/merge\z})
      unless pull_request || (@ref == @dispatch_ref && @sha == @dispatch_sha)
        raise Error, 'push revision and dispatch identity differ; using hosted fallback'
      end
      unless @dispatch_ref.start_with?('refs/heads/')
        raise Error, 'dispatch requires a source branch ref; using hosted fallback'
      end
      # Separate argv items preserve every legal Git branch name without a shell.
      _stdout, _stderr, status = Open3.capture3('git', 'check-ref-format', @dispatch_ref)
      unless status.success?
        raise Error, "source branch ref validation failed (git exit #{status.exitstatus}); using hosted fallback"
      end
      raise Error, 'invalid expected SHA' unless @sha.match?(/\A[0-9a-f]{40}\z/)
      raise Error, 'invalid workflow SHA' unless @dispatch_sha.match?(/\A[0-9a-f]{40}\z/)
      raise Error, 'invalid worker platform' unless PLATFORMS.key?(@os)
      unless positive_id?(@parent_id) && positive_id?(@parent_attempt) &&
             @request_id.match?(/\A[1-9][0-9]*-[1-9][0-9]*-#{Regexp.escape(@os)}-[0-9a-f]{32}\z/)
        raise Error, 'invalid parent run identity'
      end
    end

    def positive_id?(value)
      value.to_s.match?(/\A[1-9][0-9]*\z/)
    end

    def base
      "/repos/#{@repo}/actions"
    end

    def workflow_base
      "#{base}/workflows/#{File.basename(WORKFLOW)}"
    end

    def branch
      @dispatch_ref.delete_prefix('refs/heads/')
    end

    def now
      @clock.call
    end

    def exhausted?
      @execution_start ? now - @execution_start >= EXECUTION_SECONDS : now - @queue_start >= QUEUE_SECONDS
    end

    def timeout_reason
      @execution_start ? 'self-hosted execution exceeded 3600 seconds; using hosted fallback' :
                         'self-hosted queue exceeded 300 seconds; using hosted fallback'
    end

    def discover
      query = URI.encode_www_form(event: 'workflow_dispatch', branch: branch, per_page: 100)
      runs = @api.request(:get, "#{workflow_base}/runs?#{query}").data.fetch('workflow_runs')
      matching = runs.select { |run| run['display_title'] == "ci-worker-#{@request_id}" }
      raise Error, 'ambiguous child identity; using hosted fallback' if matching.length > 1

      matching.first
    end

    def child
      run = if @child_id
              @api.request(:get, "#{base}/runs/#{@child_id}").data
            else
              discover
            end
      return unless run

      verify_run!(run)
      @child_id = run.fetch('id').to_i
      run
    end

    def verify_run!(run)
      valid = positive_id?(run['id']) && run['id'].to_i != @parent_id.to_i &&
              (!@child_id || run['id'].to_i == @child_id) &&
              run['event'] == 'workflow_dispatch' && run['head_branch'] == branch && run['head_sha'] == @dispatch_sha &&
              run['display_title'] == "ci-worker-#{@request_id}" && run['path'] == WORKFLOW &&
              run['run_attempt'] == 1 && run.dig('repository', 'full_name') == @repo
      raise Error, 'child identity or HEAD mismatch; refusing to trust or cancel it; using hosted fallback' unless valid
    end

    def worker_job(run)
      data = @api.request(:get, "#{base}/runs/#{run.fetch('id')}/attempts/1/jobs?per_page=100").data
      jobs = data.fetch('jobs')
      raise Error, 'unexpected worker job pagination' unless data.fetch('total_count') == jobs.length

      matches = jobs.select { |job| job['name'] == "CI self-hosted worker (#{@os})" }
      raise Error, 'ambiguous worker job' if matches.length > 1
      return if matches.empty?

      job = matches.first
      valid = job['run_id'] == @child_id && job['head_sha'] == @dispatch_sha && job['run_attempt'] == 1 &&
              (PLATFORMS.fetch(@os) - job.fetch('labels')).empty?
      raise Error, 'worker job identity mismatch' unless valid

      job
    end

    def parent_active!
      run = @api.request(:get, "#{base}/runs/#{@parent_id}").data
      if run['status'] == 'completed' || run['conclusion'] == 'cancelled'
        raise ParentCancelled, 'parent is no longer active'
      end
    end

    def watch
      loop do
        return [false, timeout_reason] if exhausted?

        parent_active!
        run = child
        if run
          job = worker_job(run)
          return [false, timeout_reason] if exhausted?
          if job && %w[in_progress completed].include?(job['status'])
            @execution_start ||= now
          end
          if run['status'] == 'completed'
            return [true, 'verified complete self-hosted suite; hosted repetition not needed'] if suite_passed?(run, job)

            return [false, 'self-hosted job failed or complete success evidence is missing; rerunning full hosted suite']
          end
          if job && job['status'] == 'completed' && job['conclusion'] != 'success'
            return [false, 'self-hosted job failed; rerunning full hosted suite']
          end
        end
        @sleeper.call(POLL_SECONDS)
      end
    end

    def suite_passed?(run, job)
      return false unless run['conclusion'] == 'success' && job && job['status'] == 'completed' &&
                          job['conclusion'] == 'success' && positive_id?(job['runner_id']) && !job['runner_name'].to_s.empty?

      steps = job.fetch('steps')
      return false unless steps.all? { |step| step['status'] == 'completed' && %w[success skipped].include?(step['conclusion']) }

      REQUIRED_STEPS.all? do |name|
        matches = steps.select { |step| step['name'] == name }
        matches.length == 1 && matches.first['conclusion'] == 'success'
      end
    end

    def cleanup
      if @parent_cancelled
        cleanup_parent
      else
        begin
          cleanup_ordinary
        rescue ParentCancelled
          # A first cancellation signal may arrive during ordinary cleanup.
          @parent_cancelled = true
          cleanup_parent
        end
      end
    end

    def cleanup_parent
      # GitHub sends INT, then TERM at 7.5s and KILL 2.5s later. Bound the whole
      # sequence, including slow API reads, and force immediately after verifying.
      Timeout.timeout(PARENT_CLEANUP_SECONDS, CleanupTimeout) do
        run = child
        unless run
          @cleanup_note = 'child not discoverable; no cancellation confirmed (the API may have accepted dispatch)'
          return
        end
        if run['status'] == 'completed'
          @cleanup_note = 'child already completed; original logs retained'
          return
        end
        @api.request(:post, "#{base}/runs/#{@child_id}/force-cancel")
        run = child
        @cleanup_note = if run['status'] == 'completed'
                          'child cancellation/completion confirmed; original logs retained'
                        else
                          'force-cancel requested, but completion was not confirmed'
                        end
      end
    rescue CleanupTimeout
      @cleanup_note = 'parent cleanup budget exhausted; cancellation/completion was not confirmed'
    rescue StandardError => error
      detail = error.is_a?(Error) ? error.message : error.class.to_s
      @cleanup_note = "cleanup could not be confirmed: #{detail}"
    end

    def cleanup_ordinary
      # Re-read and re-verify before *each* mutation. Never cancel the parent,
      # a rerun, a different SHA, another platform or a similarly named run.
      run = child
      unless run
        @cleanup_note = 'child not discoverable; no cancellation confirmed (the API may have accepted dispatch)'
        return
      end
      if run['status'] == 'completed'
        @cleanup_note = 'child already completed; original logs retained'
        return
      end
      started = now
      forced = false
      begin
        @api.request(:post, "#{base}/runs/#{@child_id}/cancel")
      rescue ApiError => error
        raise unless error.status == 409

        # A normal cancellation can be refused while the child is still alive.
        # Re-verify identity before escalating, even in this conflict path.
        run = child
        if run['status'] == 'completed'
          @cleanup_note = 'child already completed; original logs retained'
          return
        end
        @api.request(:post, "#{base}/runs/#{@child_id}/force-cancel")
        forced = true
      end
      loop do
        @sleeper.call([POLL_SECONDS, CLEANUP_SECONDS - (now - started)].min.clamp(0, CLEANUP_SECONDS))
        run = child
        if run['status'] == 'completed'
          @cleanup_note = 'child cancellation/completion confirmed; original logs retained'
          return
        end
        unless forced
          @api.request(:post, "#{base}/runs/#{@child_id}/force-cancel")
          forced = true
          next
        end
        if now - started >= CLEANUP_SECONDS
          @cleanup_note = 'cancel and force-cancel requested, but completion was not confirmed'
          return
        end
      end
    rescue ParentCancelled
      raise
    rescue StandardError => error
      detail = error.is_a?(Error) ? error.message : error.class.to_s
      @cleanup_note = "cleanup could not be confirmed: #{detail}"
    end
  end

  def self.with_signal_handlers(coordinator)
    previous = {}
    %w[TERM INT].each { |signal| previous[signal] = Signal.trap(signal) { coordinator.parent_cancel! } }
    yield
  ensure
    previous.each { |signal, handler| Signal.trap(signal, handler) }
  end

  def self.run(env = ENV)
    api = Api.new(token: env.fetch('GITHUB_TOKEN'), origin: env.fetch('GITHUB_API_URL', 'https://api.github.com'))
    coordinator = Coordinator.new(api: api, repo: env.fetch('GITHUB_REPOSITORY'), ref: env.fetch('GITHUB_REF'),
                                  sha: env.fetch('GITHUB_SHA'), os: env.fetch('CI_WORKER_OS'),
                                  dispatch_ref: env.fetch('CI_DISPATCH_REF', env.fetch('GITHUB_REF')),
                                  dispatch_sha: env.fetch('CI_DISPATCH_SHA', env.fetch('GITHUB_SHA')),
                                  parent_id: env.fetch('GITHUB_RUN_ID'), parent_attempt: env.fetch('GITHUB_RUN_ATTEMPT'))
    result = with_signal_handlers(coordinator) { coordinator.attempt }
    File.open(env.fetch('GITHUB_OUTPUT'), 'a') { |file| file.puts "self_hosted_passed=#{result.passed}" }
    summary = ["### Self-hosted attempt: #{result.passed ? 'verified success' : 'hosted fallback'}",
               result.reason, "Cleanup: #{result.cleanup}"]
    if result.child_id
      summary << "Worker logs: #{env.fetch('GITHUB_SERVER_URL', 'https://github.com')}/#{env.fetch('GITHUB_REPOSITORY')}/actions/runs/#{result.child_id}"
    end
    File.open(env.fetch('GITHUB_STEP_SUMMARY'), 'a') { |file| file.puts summary.join("\n\n") }
    puts summary.join("\n")
  end
end

CiSelfHostedAttempt.run if $PROGRAM_NAME == __FILE__

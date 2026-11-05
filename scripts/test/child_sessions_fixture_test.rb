#!/usr/bin/env ruby
# frozen_string_literal: true

require 'minitest/autorun'
require 'minitest/mock'
require 'rbconfig'
require_relative '../verify_child_sessions'

# These tests use only Ruby subprocesses; they never execute BIN or start a Daemon.
class ChildSessionsFixtureTest < Minitest::Test
  PIPE_PRESSURE_BYTES = 128 * 1024

  def test_capture_drains_both_streams_and_preserves_exit_status
    program = "STDOUT.write('o' * #{PIPE_PRESSURE_BYTES}); STDERR.write('e' * #{PIPE_PRESSURE_BYTES}); exit 3"
    output, error, status = child_capture({}, RbConfig.ruby, '-e', program)

    assert_equal 'o' * PIPE_PRESSURE_BYTES, output
    assert_equal 'e' * PIPE_PRESSURE_BYTES, error
    assert_equal 3, status.exitstatus
  end

  def test_timeout_reaps_owned_process_without_interrupting_readers
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    error = assert_raises(RuntimeError) do
      child_capture({}, RbConfig.ruby, '-e', 'sleep 10', seconds: 0.05)
    end

    assert_match(/CLI pid=(\d+)/, error.message)
    pid = error.message[/CLI pid=(\d+)/, 1].to_i
    assert_raises(Errno::ESRCH) { Process.kill(0, pid) }
    assert_operator Process.clock_gettime(Process::CLOCK_MONOTONIC) - started, :<, 4
  end

  def test_inherited_output_pipe_cannot_outlive_command_deadline
    # Explicit standard-stream inheritance works without fork on Windows too.
    program = "Process.spawn(RbConfig.ruby, '-e', 'sleep 1', out: STDOUT, err: STDERR); exit!"
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)

    assert_raises(RuntimeError) do
      child_capture({}, RbConfig.ruby, '-rrbconfig', '-e', program, seconds: 0.1)
    end
    assert_operator Process.clock_gettime(Process::CLOCK_MONOTONIC) - started, :<, 1
  end

  def test_wait_polls_without_asynchronous_timeout
    attempts = 0

    assert_equal :done, wait_for('deterministic poll', 2) {
      attempts += 1
      attempts == 2 ? :done : false
    }
    assert_equal 2, attempts
  end

  def test_wait_rejects_truthy_value_returned_after_deadline
    now = 100.0
    Process.stub(:clock_gettime, ->(_clock) { now }) do
      error = assert_raises(RuntimeError) do
        wait_for('late success', 1) do
          now += 2
          :done
        end
      end

      assert_equal 'Timed out waiting for late success', error.message
    end
  end
end

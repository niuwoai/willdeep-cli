# frozen_string_literal: true

require 'json'
require 'open3'
require 'timeout'

module ModelEval
  # 评测收尾。
  #
  # 一个任务的 `willdeep run` 会顺手拉起它自己那个临时 home 的 Runtime Daemon，
  # 而这个进程是脱离进程组启动的：任务超时时杀掉的进程组打不到它。
  # 它一直擦着 `<home>/runtime` 并每几秒重写一次租约，于是直接删目录会在
  # `rmdir` 上报 `Directory not empty`。那个异常从 `ensure` 里冒出来，整段清理就停了，
  # 目录和 daemon 一起留下。
  #
  # 所以先逐个 home 走正常的 `daemon stop`，停不掉再按它自己写下的 pid 发信号，
  # 然后再删目录。删不掉只记一笔，不让它把已经跑完的这一轮带崩。
  module Cleanup
    # 信号发出后等它收尾的上限。daemon 的关闭有固定宽限，超过就是卡住了。
    SIGNAL_WAIT_SECONDS = 8

    module_function

    # 返回没能删掉的目录及原因。空数组表示干净。
    def release(run_root, binary)
      return [] unless run_root && File.directory?(run_root)

      leftover = []
      daemon_homes(run_root).each do |home|
        stop_daemon(home, binary)
      rescue StandardError => e
        leftover << "#{home}：停掉 daemon 失败（#{e.message}）"
      end
      begin
        FileUtils.remove_entry(run_root)
      rescue StandardError => e
        leftover << "#{run_root}：#{e.message}"
      end
      leftover
    end

    # 只认 `<home>/runtime/daemon.json`。任务自己的代码里出现同名文件不算。
    def daemon_homes(run_root)
      Dir.glob(File.join(run_root, '*', '*', 'home')).select do |home|
        File.file?(File.join(home, 'runtime', 'daemon.json'))
      end
    end

    def stop_daemon(home, binary)
      state_path = File.join(home, 'runtime', 'daemon.json')
      return unless File.file?(state_path)

      _output, status = run_cli(binary, home, %w[daemon stop])
      return if status&.success? && !File.file?(state_path)

      # 命令没有收掉它：按它启动时写下的 pid 发信号。pid 必须对得上这个 home，
      # 否则不动——擅自杀掉一个被复用的进程号比留下它更糟。
      pid = daemon_pid(state_path)
      return unless pid && owns_home?(pid, home)

      signal_and_wait(pid, 'TERM')
      signal_and_wait(pid, 'KILL') if process_alive?(pid)
    end

    def run_cli(binary, home, args)
      Open3.capture2e({ 'WILLDEEP_HOME' => home }, binary, *args)
    rescue Errno::ENOENT
      ['', nil]
    end

    def daemon_pid(state_path)
      pid = Integer(JSON.parse(File.read(state_path))['pid'], exception: false)
      pid&.positive? ? pid : nil
    rescue JSON::ParserError, Errno::ENOENT
      nil
    end

    # 进程环境里的 WILLDEEP_HOME 指向这个目录，才算是它的 daemon。
    def owns_home?(pid, home)
      output, status = Open3.capture2e('ps', '-Eww', '-o', 'command=', '-p', pid.to_s)
      status.success? && output.include?("WILLDEEP_HOME=#{home}")
    rescue Errno::ENOENT
      false
    end

    def signal_and_wait(pid, signal)
      begin
        Process.kill(signal, pid)
      rescue Errno::ESRCH
        return
      end
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + SIGNAL_WAIT_SECONDS
      sleep 0.2 while process_alive?(pid) && Process.clock_gettime(Process::CLOCK_MONOTONIC) < deadline
    end

    # `kill 0` 对一个已经退出、但还没被 `wait` 回收的进程也返回成功，会把死进程
    # 当成活的。能 `waitpid` 到就说明是自己的子进程且已经退出。不是自己的子进程时
    # `waitpid` 报 `ECHILD`，退回 `kill 0`。
    def process_alive?(pid)
      return false if Process.waitpid(pid, Process::WNOHANG)

      Process.kill(0, pid)
      true
    rescue Errno::ESRCH
      false
    rescue Errno::EPERM, Errno::ECHILD
      kill_probe(pid)
    end

    def kill_probe(pid)
      Process.kill(0, pid)
      true
    rescue Errno::ESRCH
      false
    rescue Errno::EPERM
      true
    end
  end
end

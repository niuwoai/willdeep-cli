require "pty"
require "io/console"
require "json"
require "fileutils"
require "digest"
require "open3"
require "base64"
binary = File.expand_path(ARGV.fetch(0))
fixture = JSON.parse(File.read(ARGV.fetch(1)))
raise "not an isolated team fixture" unless fixture.fetch("home").start_with?("/private/tmp/willdeep-team-runtime-")
captured = "".b
chunks = []
env = { "WILLDEEP_HOME" => fixture.fetch("home"), "WILLDEEP_TELEMETRY_DISABLED" => "1", "TERM" => "xterm-256color", "TERM_PROGRAM" => "fixture" }
PTY.spawn(env, binary, "--config", fixture.fetch("config"), "--workspace", fixture.fetch("workspace"), "--resume", fixture.fetch("session_id"), "--language", "en") do |reader, writer, pid|
  reader.winsize = [32, 88]
  start = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  stage = 0
  plan = [[2, "/sidebar\r"], [3, "/agent team #{fixture.fetch('agent_id')}\r"]]
  8.times { |index| plan << [4 + index * 0.3, "\e[5~"] }
  plan << [7, "/agent runs #{fixture.fetch('agent_id')}\r"]
  4.times { |index| plan << [8 + index * 0.3, "\e[5~"] }
  begin
    loop do
      elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - start
      if IO.select([reader], nil, nil, 0.05)
        data = reader.read_nonblock(1_000_000, exception: false)
        break if data.nil?
        if data != :wait_readable
          captured << data
          chunks << Base64.strict_encode64(data)
          writer.write("\e[6;18;9t\e[0n") if data.include?("\e[16t") || data.include?("\e[5n")
        end
      end
      if stage < plan.length && elapsed >= plan[stage][0]
        writer.write(plan[stage][1]); stage += 1
      end
      break if elapsed > 11
      raise "TUI fixture startup timed out" if elapsed > 15
    end
  rescue Errno::EIO
  ensure
    Process.kill("TERM", pid) rescue Errno::ESRCH
    Process.wait(pid) rescue Errno::ECHILD
  end
end
chunks_path = "/private/tmp/willdeep-team-tui-chunks.json"
File.write(chunks_path, JSON.generate(chunks))
snapshot, error, status = Open3.capture3("node", File.join(__dir__, "runtime-team-terminal.mjs"), chunks_path)
raise "terminal emulator failed: #{error}" unless status.success?
rendered = JSON.parse(snapshot)
File.write(File.join(__dir__, "runtime-team-tui.txt"), rendered.fetch("screen"))
raise "team usage or Run state was not rendered: #{rendered.fetch('matched')}" unless rendered.fetch("matched").values.all?
File.write(File.join(__dir__, "runtime-team-tui.json"), JSON.pretty_generate(status: "passed", columns: 88, rows: 32, checks: ["真实 PTY 中打开 Runtime 侧栏", "团队统计与 Run 历史命令在同一会话生效", "预算阻塞与累计用量可查看"], binary_sha256: Digest::SHA256.file(binary).hexdigest) + "\n")
puts JSON.generate(status: "passed", checks: 3)

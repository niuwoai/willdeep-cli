#!/usr/bin/env ruby
# Isolated Provider switching: OpenAI, Anthropic, foreground, Runtime and child Agents.
require 'socket'
require 'json'
require 'tmpdir'
require 'fileutils'
require 'pty'
require 'io/console'

binary = ARGV.fetch(0)
root = Dir.mktmpdir('wd-model-provider-', '/private/tmp')
home = File.join(root, 'home')
workspace = File.join(root, 'workspace')
FileUtils.mkdir_p([home, workspace])
server = TCPServer.new('127.0.0.1', 0)
port = server.addr[1]
config = File.join(home, 'config.toml')
File.write(config, "version = 1\ndefault_provider = \"alpha\"\n[agent]\nmax_turns = 5\napproval = \"full\"\nauto_title = false\ninput_suggestions = false\nsmall_model_routing = false\nauto_dispatch_read_only = false\nsafety_judge = false\n[providers.alpha]\nprovider = \"openai-compatible\"\napi = \"chat-completions\"\napi_base = \"http://127.0.0.1:#{port}/alpha/v1\"\napi_key = \"alpha-test-only\"\nmodel = \"alpha-model\"\n[providers.beta]\nprovider = \"anthropic\"\napi = \"anthropic-messages\"\napi_base = \"http://127.0.0.1:#{port}/beta\"\napi_key = \"beta-test-only\"\nmodel = \"beta-model\"\n")
File.chmod(0600, config)
records=[]
lock=Mutex.new
Thread.new do
  loop do
    socket=server.accept
    Thread.new(socket) do |s|
      begin
        method,path=s.gets.split(' ',3)
        headers={}
        while (line=s.gets) && line!="\r\n"
          key,value=line.split(':',2);headers[key.downcase]=value.strip if value
        end
        body=s.read(headers.fetch('content-length','0').to_i)
        request=body.empty? ? {} : JSON.parse(body)
        beta=path.start_with?('/beta')
        messages=request.fetch('messages',[])
        users=messages.select{|m|m['role']=='user'}.map{|m|m['content'].to_s}
        child=users.last.to_s.include?('child-provider-check') && !users.last.to_s.include?('delegate-provider-check')
        lock.synchronize do
          records << {method:method,path:path,model:request['model'],auth_ok:beta ? headers['x-api-key']=='beta-test-only' && !headers.key?('authorization') : headers['authorization']=='Bearer alpha-test-only',child:child,users:users}
          File.write(File.join(root,'requests.json'),JSON.pretty_generate(records))
        end
        model=beta ? 'beta-model' : 'alpha-model'
        if method=='GET'
          response={data:[{id:model}]}.to_json
          s.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{response.bytesize}\r\nConnection: close\r\n\r\n#{response}")
          next
        end
        spawn=users.last.to_s.include?('delegate-provider-check') && !child && !messages.last.to_json.include?('tool_result')
        args={prompt:'child-provider-check; return a short confirmation without changing files',profile:'reviewer',task:{goal:'Read-only provider verification',read_files:[],write_files:[],verifier:{command:'true'}}}
        if beta
          content=spawn ? [{type:'tool_use',id:'call_child',name:'spawn_agent',input:args}] : [{type:'text',text:child ? 'child provider verified' : 'provider verified'}]
          if request['stream']
            s.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
            emit=->(type,data){s.write("event: #{type}\ndata: #{data.merge(type:type).to_json}\n\n")}
            emit.call('message_start',{message:{id:'fixture-msg',type:'message',role:'assistant',model:model,content:[],usage:{input_tokens:10,output_tokens:0}}})
            block=content.first
            emit.call('content_block_start',{index:0,content_block:spawn ? block.merge(input:{}) : {type:'text',text:''}})
            emit.call('content_block_delta',{index:0,delta:spawn ? {type:'input_json_delta',partial_json:args.to_json} : {type:'text_delta',text:block[:text]}})
            emit.call('content_block_stop',{index:0})
            emit.call('message_delta',{delta:{stop_reason:spawn ? 'tool_use' : 'end_turn'},usage:{output_tokens:8}})
            emit.call('message_stop',{})
          else
            response={id:'fixture-msg',type:'message',role:'assistant',model:model,content:content,stop_reason:spawn ? 'tool_use' : 'end_turn',usage:{input_tokens:10,output_tokens:8}}.to_json
            s.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{response.bytesize}\r\nConnection: close\r\n\r\n#{response}")
          end
        else
          if request['stream']
            s.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
            s.write("data: #{({id:'fixture',model:model,choices:[{index:0,delta:{content:'alpha provider verified'},finish_reason:'stop'}],usage:{prompt_tokens:10,completion_tokens:5,total_tokens:15}}).to_json}\n\ndata: [DONE]\n\n")
          else
            response={id:'fixture',model:model,choices:[{message:{role:'assistant',content:'alpha provider verified'},finish_reason:'stop'}],usage:{prompt_tokens:10,completion_tokens:5,total_tokens:15}}.to_json
            s.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{response.bytesize}\r\nConnection: close\r\n\r\n#{response}")
          end
        end
      rescue Errno::EPIPE, Errno::ECONNRESET
      ensure
        s.close
      end
    end
  end
rescue IOError
end
env={'WILLDEEP_HOME'=>home,'TERM'=>'xterm-256color','WILLDEEP_LOCAL_DISCOVERY'=>'0','NO_PROXY'=>'127.0.0.1,localhost','no_proxy'=>'127.0.0.1,localhost','WILLDEEP_API_KEY'=>nil,'WILLDEEP_API_BASE'=>nil,'WILLDEEP_MODEL'=>nil,'OPENAI_API_KEY'=>nil,'ANTHROPIC_API_KEY'=>nil,'OPENAI_BASE_URL'=>nil,'ANTHROPIC_BASE_URL'=>nil}
# At 100x30 the 100x24 cascade starts at row 4. Provider and model
# entries start at row 8; selecting a Provider previews without committing.
plan=[[3,'/model'],[3.4,"\r"],[5,"\x10"],[5.5,"\e[B"],[6,"\r"],[8,"\r"],[10,'/local provider-local-check'],[10.4,"\r"],[14,'delegate-provider-check'],[14.4,"\r"],[18,'/local delegate-provider-check'],[18.4,"\r"],[22,'/model'],[22.4,"\r"],[24,"\e[<0;6;8M\e[<0;6;8m"],[25,"\e[<0;34;8M\e[<0;34;8m"],[29,'/local after-mouse-check'],[29.4,"\r"],[33,"\x03"]]
bytes=''.b;stage=0;start=Process.clock_gettime(Process::CLOCK_MONOTONIC)
begin
  PTY.spawn(env,binary,'--config',config,'--workspace',workspace,'--language','en') do |reader,writer,pid|
    reader.winsize=[30,100]
    begin
      loop do
        if IO.select([reader],nil,nil,0.03)
          part=reader.read_nonblock(1_000_000,exception:false);break if part.nil?
          unless part==:wait_readable
            bytes<<part
            writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
          end
        end
        elapsed=Process.clock_gettime(Process::CLOCK_MONOTONIC)-start
        if stage<plan.length && elapsed>=plan[stage][0]
          File.binwrite(File.join(root,"stage-#{stage}.ansi"),bytes)
          writer.write(plan[stage][1]);stage+=1
        end
        break if elapsed>36
      end
    rescue Errno::EIO
    ensure
      Process.kill('TERM',pid) rescue Errno::ESRCH
      Process.wait(pid) rescue Errno::ECHILD
    end
  end
ensure
  system(env,binary,'daemon','stop',out:File::NULL,err:File::NULL)
  server.close
end
File.binwrite(File.join(root,'all.ansi'),bytes)
sessions=Dir[File.join(home,'sessions','*.json')].map{|p|JSON.parse(File.read(p))}
report={root:root,stages:stage,requests:records.map{|r|r.reject{|k,_|k==:users}},sessions:sessions.map{|s|s.slice('profile','model')},beta_local:records.any?{|r|r[:path]=='/beta/v1/messages' && r[:users].to_s.include?('provider-local-check')},beta_child:records.any?{|r|r[:path]=='/beta/v1/messages' && r[:child]},alpha_after_mouse:records.any?{|r|r[:path]=='/alpha/v1/chat/completions' && r[:users].to_s.include?('after-mouse-check')},all_auth_correct:records.all?{|r|r[:auth_ok]}}
report[:runtime_sessions]=JSON.parse(File.read(File.join(home,'runtime','sessions.json'))).map{|s|s.slice('profile','model','status')}
report[:beta_child_requests]=records.count{|r|r[:child] && r[:path]=='/beta/v1/messages'}
File.write(File.join(root,'report.json'),JSON.pretty_generate(report))
puts report.to_json
raise "Provider switch verification failed: #{root}" unless report.values_at(:beta_local,:beta_child,:alpha_after_mouse,:all_auth_correct).all? && report[:beta_child_requests] >= 2 && sessions.all?{|s|s['profile']=='alpha' && s['model']=='alpha-model'} && report[:runtime_sessions].all?{|s|s['profile']=='alpha' && s['model']=='alpha-model'}

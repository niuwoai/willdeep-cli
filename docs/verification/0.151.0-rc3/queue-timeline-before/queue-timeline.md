# TUI 队列时间线验收

willdeep 0.151.0-rc2

通过：false；检查：1。

| 布局 | 检查 | 结果 |
|---|---|---|
| compact | queued inputs do not start early | PASS |
| compact | first dequeued user follows the complete preceding reply | FAIL |

隔离：Private HOME/WILLDEEP_HOME/workspace, loopback synthetic provider, captured OSC 52 only.

仅覆盖本机合成 Provider 和 Runtime TUI，不代表真实模型、跨平台或 /local 的端到端验证。

RuntimeError: wrong first dequeue order:  hugo: QUEUE_TIMELINE_READY
                
                                                                      [started] FIRST_TIMELINE_REQUEST · now 
                      
                                                                         [started] QUEUED_TIMELINE_ONE · now 
                         
 System: Runtime accepted and queued the message; waiting to start                                           
 System: The current turn is still running; no new progress received yet
                                                                  
                                                               [accepted · queued] QUEUED_TIMELINE_TWO · now 
                                                                                                         
 System: Runtime accepted and queued the message; waiting to start                                           
 hugo: FIRST_TIMELINE_HEAD
 FIRST_TIMELINE_TAIL · now · fixture                              
                                               
 ── turn finished · first reply 0.44s · total 1.04s · in 1.00K · out 100 · sending queued messages next ·
 your turn ── · your turn
 hugo · replying: SECOND_TIMELINE_REPLY



















▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
 UNSENT_TIMELINE_DRAFT                                                                                        
                                                                                                              
                                                                                                              
◐ Receiving reply · elapsed 0.3s · Focus: Prompt · context 0% · latest ↑1.00K ↓100 · queued 2 · Esc Esc×2 inte

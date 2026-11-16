// Run from the local Multica server module: go run /path/to/this/file.go adapter stub [resume].
// This exercises the real agent.New("claude") parser; it does not run a Multica daemon.
package main

import (
	"context"
	"encoding/json"
	"io"
	"log/slog"
	"os"
	"time"

	"github.com/multica-ai/multica/server/pkg/agent"
)

func main() {
	if len(os.Args) < 3 {
		panic("usage: contract.go adapter stub [resume]")
	}
	backend, err := agent.New("claude", agent.Config{
		ExecutablePath: os.Args[1],
		Env:            map[string]string{"WILLDEEP_BIN": os.Args[2]},
		Logger:         slog.New(slog.NewTextHandler(io.Discard, nil)),
	})
	if err != nil {
		panic(err)
	}
	options := agent.ExecOptions{Model: "requested-model", Timeout: 15 * time.Second}
	if len(os.Args) > 3 {
		options.ResumeSessionID = os.Args[3]
	}
	session, err := backend.Execute(context.Background(), "fix it", options)
	if err != nil {
		panic(err)
	}
	messages := make([]agent.Message, 0)
	for message := range session.Messages {
		messages = append(messages, message)
	}
	result := <-session.Result
	if err := json.NewEncoder(os.Stdout).Encode(struct {
		Messages []agent.Message `json:"messages"`
		Result   agent.Result    `json:"result"`
	}{messages, result}); err != nil {
		panic(err)
	}
}

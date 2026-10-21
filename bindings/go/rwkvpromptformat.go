// Package rwkvpf is the Go binding for the RWKV G1/G1x prompt format checker
// and auto-fixer.
//
// It talks to the rwkv-prompt-format JSON CLI, so it needs no cgo, no C
// toolchain and no shared-library discovery: build the CLI once and put it on
// PATH (or set RWKV_PROMPT_FORMAT_CLI to its full path).
//
//	cd rwkv-prompt-format && cargo build --release
//	export RWKV_PROMPT_FORMAT_CLI=$PWD/target/release/rwkv-prompt-format
//
// Every call spawns the CLI (about a millisecond of overhead); for hot loops
// use the C ABI directly.
package rwkvpf

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
)

// Fix flags, mirroring RWKV_PF_FIX_* in the C header and the CLI.
const (
	FixSafe          uint32 = 0x1
	FixStructural    uint32 = 0x2
	FixStripMetadata uint32 = 0x4
	FixAll                  = FixSafe | FixStructural
)

// Severity is "error", "warn" or "info".
type Severity string

// Finding is one checker conclusion. Line is 1-based; 0 means whole-text.
type Finding struct {
	Code     string   `json:"code"`
	Severity Severity `json:"severity"`
	Line     int      `json:"line"`
	Message  string   `json:"message"`
	Evidence string   `json:"evidence"`
}

// GenerationPoint is where the model is expected to continue.
type GenerationPoint struct {
	Line   int    `json:"line"`
	Role   string `json:"role"`
	Seeded bool   `json:"seeded"`
}

// FormatStats holds checkable text statistics.
type FormatStats struct {
	Chars           int  `json:"chars"`
	Lines           int  `json:"lines"`
	UserBlocks      int  `json:"user_blocks"`
	AssistantBlocks int  `json:"assistant_blocks"`
	SystemBlocks    int  `json:"system_blocks"`
	Instruction     int  `json:"instruction_blocks"`
	InputBlocks     int  `json:"input_blocks"`
	ResponseBlocks  int  `json:"response_blocks"`
	BlankSeparators int  `json:"blank_separators"`
	MaxBlankRun     int  `json:"max_blank_run"`
	MetadataLines   int  `json:"metadata_lines"`
	ThinkPrefix     bool `json:"think_prefix"`
	FunctionCall    bool `json:"function_call"`
}

// FormatReport is the result of Check.
type FormatReport struct {
	Profile         string           `json:"profile"`
	Format          string           `json:"format"`
	OK              bool             `json:"ok"`
	ErrorCount      int              `json:"error_count"`
	WarningCount    int              `json:"warning_count"`
	InfoCount       int              `json:"info_count"`
	GenerationPoint *GenerationPoint `json:"generation_point"`
	Stats           FormatStats      `json:"stats"`
	Findings        []Finding        `json:"findings"`
}

// FixOptions records which transformations were enabled.
type FixOptions struct {
	Safe          bool `json:"safe"`
	Structural    bool `json:"structural"`
	StripMetadata bool `json:"strip_metadata"`
}

// AppliedFix is one class of problems removed by Fix.
type AppliedFix struct {
	Code    string `json:"code"`
	Count   int    `json:"count"`
	Message string `json:"message"`
}

// FixResult is the result of Fix. Text is the fixed prompt.
type FixResult struct {
	Changed bool         `json:"changed"`
	Options FixOptions   `json:"options"`
	Applied []AppliedFix `json:"applied"`
	Notes   []string     `json:"notes"`
	Before  FormatReport `json:"before"`
	After   FormatReport `json:"after"`
	Text    string       `json:"text"`
}

// CLIPath returns RWKV_PROMPT_FORMAT_CLI, or "rwkv-prompt-format" so the
// system PATH decides. Override it when the binary is not installed globally.
func CLIPath() string {
	if path := os.Getenv("RWKV_PROMPT_FORMAT_CLI"); path != "" {
		return path
	}
	return "rwkv-prompt-format"
}

func run(args []string, stdin string) ([]byte, error) {
	cmd := exec.Command(CLIPath(), args...)
	cmd.Stdin = strings.NewReader(stdin)
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()
	if err != nil {
		var exitErr *exec.ExitError
		// Exit code 1 means "findings remain", not a transport failure; the
		// JSON on stdout is still valid. Anything else is an error.
		if !errors.As(err, &exitErr) || exitErr.ExitCode() != 1 || stdout.Len() == 0 {
			detail := strings.TrimSpace(stderr.String())
			if detail == "" {
				detail = err.Error()
			}
			return nil, fmt.Errorf("rwkv-prompt-format: %s", detail)
		}
	}
	return stdout.Bytes(), nil
}

// Check runs the checker. It returns an error only on transport/JSON failures;
// format problems are reported in the result (report.OK == false).
func Check(text string) (*FormatReport, error) {
	// --no-fail keeps the exit code 0 so a malformed prompt is not an error.
	raw, err := run([]string{"check", "--stdin", "--json", "--no-fail"}, text)
	if err != nil {
		return nil, err
	}
	var report FormatReport
	if err := json.Unmarshal(raw, &report); err != nil {
		return nil, fmt.Errorf("decode check report: %w", err)
	}
	return &report, nil
}

// Fix rewrites the prompt. Flags must include FixSafe, FixStructural or both.
func Fix(text string, flags uint32) (*FixResult, error) {
	args, err := fixFlags(flags)
	if err != nil {
		return nil, err
	}
	raw, err := run(append([]string{"fix", "--stdin", "--json", "--no-fail"}, args...), text)
	if err != nil {
		return nil, err
	}
	var result FixResult
	if err := json.Unmarshal(raw, &result); err != nil {
		return nil, fmt.Errorf("decode fix result: %w", err)
	}
	return &result, nil
}

// FixText returns only the fixed prompt, without the JSON round trip.
func FixText(text string, flags uint32) (string, error) {
	args, err := fixFlags(flags)
	if err != nil {
		return "", err
	}
	raw, err := run(append([]string{"fix", "--stdin", "--no-fail"}, args...), text)
	if err != nil {
		return "", err
	}
	return string(raw), nil
}

func fixFlags(flags uint32) ([]string, error) {
	safe := flags&FixSafe != 0
	structural := flags&FixStructural != 0
	if !safe && !structural {
		return nil, errors.New("fix flags must include FixSafe, FixStructural or both")
	}
	out := make([]string, 0, 2)
	if safe && !structural {
		out = append(out, "--safe-only")
	}
	if structural && !safe {
		out = append(out, "--structural-only")
	}
	if flags&FixStripMetadata != 0 {
		out = append(out, "--strip-metadata")
	}
	return out, nil
}

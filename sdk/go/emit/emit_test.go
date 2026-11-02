package emit

import (
	"encoding/json"
	"io"
	"os"
	"strings"
	"testing"

	"github.com/preloopdev/loopt/sdk/go/om"
)

func TestMarshalProducesCompactJSON(t *testing.T) {
	workflow := om.New(
		om.F("zebra", 1),
		om.F("apple", om.New(om.F("nested", true))),
	)
	encoded, err := Marshal(workflow)
	if err != nil {
		t.Fatalf("Marshal: %v", err)
	}
	if want := `{"zebra":1,"apple":{"nested":true}}`; string(encoded) != want {
		t.Fatalf("Marshal = %s, want %s", encoded, want)
	}
	if strings.Contains(string(encoded), "\n") {
		t.Fatalf("Marshal must not include a trailing newline: %q", encoded)
	}
}

func TestPrintWritesExactlyOneLine(t *testing.T) {
	workflow := om.New(
		om.F("name", "CI"),
		om.F("run", "a && b"),
		om.F("zebra", 1),
	)

	stdout := os.Stdout
	reader, writer, err := os.Pipe()
	if err != nil {
		t.Fatalf("os.Pipe: %v", err)
	}
	os.Stdout = writer
	printErr := Print(workflow)
	os.Stdout = stdout
	writer.Close()
	output, readErr := io.ReadAll(reader)
	reader.Close()
	if printErr != nil {
		t.Fatalf("Print: %v", printErr)
	}
	if readErr != nil {
		t.Fatalf("ReadAll: %v", readErr)
	}

	text := string(output)
	if strings.Count(text, "\n") != 1 || !strings.HasSuffix(text, "\n") {
		t.Fatalf("Print output must be exactly one line: %q", text)
	}
	if !strings.Contains(text, "a && b") {
		t.Fatalf("Print HTML-escaped the output: %s", text)
	}
	var parsed map[string]any
	if err := json.Unmarshal(output, &parsed); err != nil {
		t.Fatalf("Print output is not one JSON object (%v): %s", err, text)
	}
	if parsed["name"] != "CI" {
		t.Fatalf("parsed value = %#v", parsed)
	}
	if strings.Index(text, `"name"`) > strings.Index(text, `"zebra"`) {
		t.Fatalf("Print reordered fields: %s", text)
	}
}

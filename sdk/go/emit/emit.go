// Package emit writes an assembled workflow document to stdout.
package emit

import (
	"bytes"
	"encoding/json"
	"io"
	"os"
)

// Print serializes workflow to compact JSON and writes it as exactly one
// line on stdout, the shape `loopt gen` reads back from a generated
// authoring program. HTML characters are not escaped, so shell snippets and
// URLs stay readable.
func Print(workflow any) error {
	return encode(os.Stdout, workflow)
}

// Marshal serializes workflow like Print does, without a trailing newline.
func Marshal(workflow any) ([]byte, error) {
	var buf bytes.Buffer
	if err := encode(&buf, workflow); err != nil {
		return nil, err
	}
	return bytes.TrimSuffix(buf.Bytes(), []byte{'\n'}), nil
}

func encode(destination io.Writer, workflow any) error {
	enc := json.NewEncoder(destination)
	enc.SetEscapeHTML(false)
	return enc.Encode(workflow)
}

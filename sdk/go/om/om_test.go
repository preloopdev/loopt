package om

import (
	"bytes"
	"encoding/json"
	"strings"
	"testing"
)

func TestMarshalPreservesInsertionOrder(t *testing.T) {
	object := New(
		F("zebra", 1),
		F("apple", New(
			F("second", 2),
			F("first", 3),
		)),
		F("middle", []any{1, 2, 3}),
	)
	encoded, err := json.Marshal(object)
	if err != nil {
		t.Fatalf("Marshal: %v", err)
	}
	want := `{"zebra":1,"apple":{"second":2,"first":3},"middle":[1,2,3]}`
	if string(encoded) != want {
		t.Fatalf("Marshal = %s, want %s", encoded, want)
	}
	if object.Len() != 3 {
		t.Fatalf("Len() = %d", object.Len())
	}
}

func TestEmptyObjectMarshalsAsBraces(t *testing.T) {
	for _, object := range []*Object{New(), {}} {
		encoded, err := json.Marshal(object)
		if err != nil {
			t.Fatalf("Marshal: %v", err)
		}
		if string(encoded) != "{}" {
			t.Fatalf("Marshal(empty) = %s", encoded)
		}
	}
}

func TestSetReplacesInPlace(t *testing.T) {
	object := New(F("a", 1), F("b", 2))
	object.Set("b", 20).Set("c", 3)
	encoded, err := json.Marshal(object)
	if err != nil {
		t.Fatalf("Marshal: %v", err)
	}
	if want := `{"a":1,"b":20,"c":3}`; string(encoded) != want {
		t.Fatalf("Marshal = %s, want %s", encoded, want)
	}
}

func TestMarshalDoesNotEscapeHTML(t *testing.T) {
	object := New(F("run", "a && b <c>"))
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(object); err != nil {
		t.Fatalf("Encode: %v", err)
	}
	if !strings.Contains(buf.String(), "a && b <c>") {
		t.Fatalf("output HTML-escaped: %s", buf.String())
	}
}

func TestMarshalErrorNamesTheField(t *testing.T) {
	object := New(F("bad", make(chan int)))
	if _, err := json.Marshal(object); err == nil {
		t.Fatal("marshaling an unsupported value must fail")
	} else if !strings.Contains(err.Error(), `field "bad"`) {
		t.Fatalf("error = %v, want it to name the field", err)
	}
}

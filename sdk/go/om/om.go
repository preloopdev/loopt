// Package om provides an insertion-ordered JSON object.
//
// Go's built-in map[string]any marshals through encoding/json with its keys
// sorted alphabetically, which would silently reorder a workflow's top-level
// keys, a job's fields, or a matrix's axes. An Object keeps the fields in
// the order they were added and implements json.Marshaler, so a workflow
// assembled from New/F round-trips with its author order intact.
package om

import (
	"bytes"
	"encoding/json"
	"fmt"
)

// Field is one ordered key/value pair of an Object.
type Field struct {
	Key   string
	Value any
}

// F builds one ordered field for New.
func F(key string, value any) Field {
	return Field{Key: key, Value: value}
}

// Object is an insertion-ordered JSON object. The zero value is a valid,
// empty object.
type Object struct {
	fields []Field
}

// New builds an ordered object from fields, in the order given.
func New(fields ...Field) *Object {
	return &Object{fields: fields}
}

// Fields returns the object's fields in insertion order. The returned slice
// must not be modified; it aliases the object's storage.
func (o *Object) Fields() []Field {
	if o == nil {
		return nil
	}
	return o.fields
}

// Len reports the number of fields in the object.
func (o *Object) Len() int {
	if o == nil {
		return 0
	}
	return len(o.fields)
}

// Set adds a field, or replaces the value of an existing field in place,
// preserving its original position. It returns the object for chaining.
func (o *Object) Set(key string, value any) *Object {
	for i := range o.fields {
		if o.fields[i].Key == key {
			o.fields[i].Value = value
			return o
		}
	}
	o.fields = append(o.fields, Field{Key: key, Value: value})
	return o
}

// MarshalJSON writes the object's fields in insertion order instead of
// Go's default alphabetical map ordering. String keys and values are
// encoded without HTML escaping, matching emit.Print's output.
func (o *Object) MarshalJSON() ([]byte, error) {
	if o == nil || len(o.fields) == 0 {
		return []byte("{}"), nil
	}
	buf := []byte{'{'}
	for i, field := range o.fields {
		if i > 0 {
			buf = append(buf, ',')
		}
		key, err := marshal(field.Key)
		if err != nil {
			return nil, err
		}
		buf = append(buf, key...)
		buf = append(buf, ':')
		value, err := marshal(field.Value)
		if err != nil {
			return nil, fmt.Errorf("field %q: %w", field.Key, err)
		}
		buf = append(buf, value...)
	}
	buf = append(buf, '}')
	return buf, nil
}

// marshal encodes a single JSON value without HTML escaping and without the
// trailing newline json.Encoder appends.
func marshal(value any) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(value); err != nil {
		return nil, err
	}
	return bytes.TrimSuffix(buf.Bytes(), []byte{'\n'}), nil
}

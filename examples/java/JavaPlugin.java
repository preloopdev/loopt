import java.io.IOException;
import java.math.BigDecimal;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Minimal Loopt language plugin for Java.
 *
 * <p>This is a protocol reference, not a pretty-printer. It stores the workflow
 * as JSON inside a generated Java source file. A production emitter should
 * generate ordinary Java data structures and a JSON writer instead.
 *
 * <p>Run it with JDK 11+:
 *
 * <pre>
 * loopt import --language java --plugin java \
 *   --plugin-arg examples/java/JavaPlugin.java
 * </pre>
 */
public final class JavaPlugin {
    private static final int PROTOCOL_VERSION = 1;

    public static void main(String[] args) throws Exception {
        Map<String, Object> response = new LinkedHashMap<>();
        try {
            String input = new String(System.in.readAllBytes(), StandardCharsets.UTF_8);
            Object parsed = Json.parse(input);
            if (!(parsed instanceof Map)) {
                throw new IllegalArgumentException("request must be a JSON object");
            }
            @SuppressWarnings("unchecked")
            Map<String, Object> request = (Map<String, Object>) parsed;

            Object version = request.get("version");
            if (!(version instanceof Number) || ((Number) version).intValue() != PROTOCOL_VERSION) {
                throw new IllegalArgumentException("unsupported protocol version " + version);
            }

            Object op = request.get("op");
            Object result;
            if ("describe".equals(op)) {
                result = describe();
            } else if ("emit".equals(op)) {
                result = emit(params(request));
            } else if ("synth".equals(op)) {
                result = synth(params(request));
            } else {
                throw new IllegalArgumentException("unknown op " + op);
            }

            response.put("status", "ok");
            response.put("version", PROTOCOL_VERSION);
            response.put("result", result);
            write(response);
        } catch (Throwable error) {
            response.clear();
            response.put("status", "error");
            response.put("version", PROTOCOL_VERSION);
            response.put("message", message(error));
            write(response);
        }
    }

    private static Map<String, Object> describe() {
        Map<String, Object> result = new LinkedHashMap<>();
        result.put("language", "java");
        result.put("extensions", List.of("java"));
        return result;
    }

    private static Map<String, Object> emit(Map<String, Object> params) throws IOException {
        Object workflow = params.get("workflow");
        Object stem = params.get("stem");
        if (!(stem instanceof String)) {
            throw new IllegalArgumentException("emit requires a string stem");
        }

        String entrypoint = stem + ".java";
        String json = Json.stringify(workflow);
        String source = "class Workflow {\n"
                + "  public static void main(String[] args) {\n"
                + "    System.out.print(\"" + javaEscape(json) + "\");\n"
                + "  }\n"
                + "}\n";

        Map<String, Object> file = new LinkedHashMap<>();
        file.put("path", entrypoint);
        file.put("content", source);

        Map<String, Object> result = new LinkedHashMap<>();
        result.put("entrypoint", entrypoint);
        result.put("files", List.of(file));
        return result;
    }

    private static Map<String, Object> synth(Map<String, Object> params) throws Exception {
        Object source = params.get("source");
        if (!(source instanceof String)) {
            throw new IllegalArgumentException("synth requires a string source");
        }

        Process process = new ProcessBuilder("java", (String) source)
                .redirectError(ProcessBuilder.Redirect.INHERIT)
                .start();
        String stdout = new String(process.getInputStream().readAllBytes(), StandardCharsets.UTF_8);
        int status = process.waitFor();
        if (status != 0) {
            throw new IllegalStateException("java " + source + " exited with " + status);
        }

        Map<String, Object> result = new LinkedHashMap<>();
        result.put("workflow", Json.parse(stdout));
        return result;
    }

    @SuppressWarnings("unchecked")
    private static Map<String, Object> params(Map<String, Object> request) {
        Object params = request.get("params");
        if (!(params instanceof Map)) {
            throw new IllegalArgumentException("request is missing a params object");
        }
        return (Map<String, Object>) params;
    }

    private static void write(Object value) throws IOException {
        byte[] bytes = (Json.stringify(value) + "\n").getBytes(StandardCharsets.UTF_8);
        System.out.write(bytes);
        System.out.flush();
    }

    private static String message(Throwable error) {
        String text = error.getMessage();
        return text == null || text.isEmpty() ? error.getClass().getSimpleName() : text;
    }

    /** Escape a string for a Java double-quoted literal. */
    private static String javaEscape(String value) {
        StringBuilder out = new StringBuilder(value.length() + 16);
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            switch (c) {
                case '\\': out.append("\\\\"); break;
                case '"': out.append("\\\""); break;
                case '\n': out.append("\\n"); break;
                case '\r': out.append("\\r"); break;
                case '\t': out.append("\\t"); break;
                case '\b': out.append("\\b"); break;
                case '\f': out.append("\\f"); break;
                default:
                    if (c < 0x20 || c > 0x7e) {
                        out.append(String.format("\\u%04x", (int) c));
                    } else {
                        out.append(c);
                    }
            }
        }
        return out.toString();
    }

    /** Tiny JSON parser/writer so the plugin has no third-party dependencies. */
    private static final class Json {
        private static String stringify(Object value) {
            StringBuilder out = new StringBuilder();
            write(value, out);
            return out.toString();
        }

        private static void write(Object value, StringBuilder out) {
            if (value == null) {
                out.append("null");
            } else if (value instanceof String) {
                writeString((String) value, out);
            } else if (value instanceof Boolean) {
                out.append(value.toString());
            } else if (value instanceof BigDecimal) {
                out.append(((BigDecimal) value).toPlainString());
            } else if (value instanceof Double || value instanceof Float) {
                double number = ((Number) value).doubleValue();
                if (Double.isNaN(number) || Double.isInfinite(number)) {
                    throw new IllegalArgumentException("non-finite number");
                }
                out.append(Double.toString(number));
            } else if (value instanceof Number) {
                out.append(value.toString());
            } else if (value instanceof Map) {
                out.append('{');
                boolean first = true;
                for (Map.Entry<?, ?> entry : ((Map<?, ?>) value).entrySet()) {
                    if (!first) {
                        out.append(',');
                    }
                    first = false;
                    writeString(String.valueOf(entry.getKey()), out);
                    out.append(':');
                    write(entry.getValue(), out);
                }
                out.append('}');
            } else if (value instanceof Iterable) {
                out.append('[');
                boolean first = true;
                for (Object item : (Iterable<?>) value) {
                    if (!first) {
                        out.append(',');
                    }
                    first = false;
                    write(item, out);
                }
                out.append(']');
            } else {
                throw new IllegalArgumentException("cannot serialize " + value.getClass());
            }
        }

        private static void writeString(String value, StringBuilder out) {
            out.append('"');
            for (int i = 0; i < value.length(); i++) {
                char c = value.charAt(i);
                switch (c) {
                    case '"': out.append("\\\""); break;
                    case '\\': out.append("\\\\"); break;
                    case '\b': out.append("\\b"); break;
                    case '\f': out.append("\\f"); break;
                    case '\n': out.append("\\n"); break;
                    case '\r': out.append("\\r"); break;
                    case '\t': out.append("\\t"); break;
                    default:
                        if (c < 0x20) {
                            out.append(String.format("\\u%04x", (int) c));
                        } else {
                            out.append(c);
                        }
                }
            }
            out.append('"');
        }

        private static Object parse(String text) {
            Parser parser = new Parser(text);
            Object value = parser.value();
            parser.whitespace();
            if (parser.position != text.length()) {
                throw parser.error("trailing data");
            }
            return value;
        }

        private static final class Parser {
            private final String text;
            private int position;

            private Parser(String text) {
                this.text = text;
            }

            private Object value() {
                whitespace();
                if (position >= text.length()) {
                    throw error("unexpected end of JSON");
                }
                char c = text.charAt(position);
                switch (c) {
                    case '{': return object();
                    case '[': return array();
                    case '"': return string();
                    case 't': literal("true"); return Boolean.TRUE;
                    case 'f': literal("false"); return Boolean.FALSE;
                    case 'n': literal("null"); return null;
                    default:
                        if (c == '-' || (c >= '0' && c <= '9')) {
                            return number();
                        }
                        throw error("unexpected character '" + c + "'");
                }
            }

            private Map<String, Object> object() {
                Map<String, Object> result = new LinkedHashMap<>();
                position++;
                whitespace();
                if (peek('}')) {
                    position++;
                    return result;
                }
                while (true) {
                    whitespace();
                    if (!peek('"')) {
                        throw error("object key must be a string");
                    }
                    String key = string();
                    whitespace();
                    expect(':');
                    result.put(key, value());
                    whitespace();
                    if (peek('}')) {
                        position++;
                        return result;
                    }
                    expect(',');
                }
            }

            private List<Object> array() {
                List<Object> result = new ArrayList<>();
                position++;
                whitespace();
                if (peek(']')) {
                    position++;
                    return result;
                }
                while (true) {
                    result.add(value());
                    whitespace();
                    if (peek(']')) {
                        position++;
                        return result;
                    }
                    expect(',');
                }
            }

            private String string() {
                expect('"');
                StringBuilder result = new StringBuilder();
                while (position < text.length()) {
                    char c = text.charAt(position++);
                    if (c == '"') {
                        return result.toString();
                    }
                    if (c != '\\') {
                        result.append(c);
                        continue;
                    }
                    if (position >= text.length()) {
                        throw error("unterminated escape");
                    }
                    char escape = text.charAt(position++);
                    switch (escape) {
                        case '"': result.append('"'); break;
                        case '\\': result.append('\\'); break;
                        case '/': result.append('/'); break;
                        case 'b': result.append('\b'); break;
                        case 'f': result.append('\f'); break;
                        case 'n': result.append('\n'); break;
                        case 'r': result.append('\r'); break;
                        case 't': result.append('\t'); break;
                        case 'u':
                            if (position + 4 > text.length()) {
                                throw error("short unicode escape");
                            }
                            String hex = text.substring(position, position + 4);
                            position += 4;
                            try {
                                result.append((char) Integer.parseInt(hex, 16));
                            } catch (NumberFormatException error) {
                                throw error("invalid unicode escape");
                            }
                            break;
                        default:
                            throw error("invalid escape \\" + escape);
                    }
                }
                throw error("unterminated string");
            }

            private BigDecimal number() {
                int start = position;
                if (peek('-')) {
                    position++;
                }
                while (position < text.length() && Character.isDigit(text.charAt(position))) {
                    position++;
                }
                if (peek('.')) {
                    position++;
                    while (position < text.length() && Character.isDigit(text.charAt(position))) {
                        position++;
                    }
                }
                if (peek('e') || peek('E')) {
                    position++;
                    if (peek('+') || peek('-')) {
                        position++;
                    }
                    while (position < text.length() && Character.isDigit(text.charAt(position))) {
                        position++;
                    }
                }
                String literal = text.substring(start, position);
                try {
                    return new BigDecimal(literal);
                } catch (NumberFormatException error) {
                    throw error("invalid number " + literal);
                }
            }

            private void literal(String literal) {
                if (!text.startsWith(literal, position)) {
                    throw error("invalid literal");
                }
                position += literal.length();
            }

            private void whitespace() {
                while (position < text.length() && Character.isWhitespace(text.charAt(position))) {
                    position++;
                }
            }

            private boolean peek(char expected) {
                return position < text.length() && text.charAt(position) == expected;
            }

            private void expect(char expected) {
                if (!peek(expected)) {
                    throw error("expected '" + expected + "'");
                }
                position++;
            }

            private IllegalArgumentException error(String message) {
                return new IllegalArgumentException(message + " at offset " + position);
            }
        }
    }
}

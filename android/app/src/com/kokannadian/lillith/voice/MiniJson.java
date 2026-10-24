package com.kokannadian.lillith.voice;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** Tiny JSON reader (objects -> Map, arrays -> List, numbers -> Double) so the voice core runs on a plain JVM too. */
final class MiniJson {
    private final String s;
    private int i;

    private MiniJson(String s) {
        this.s = s;
    }

    static Object parse(String text) {
        MiniJson p = new MiniJson(text);
        p.ws();
        Object v = p.value();
        p.ws();
        if (p.i != p.s.length()) throw p.err("trailing data");
        return v;
    }

    private IllegalArgumentException err(String m) {
        return new IllegalArgumentException("json: " + m + " at " + i);
    }

    private void ws() {
        while (i < s.length() && " \t\r\n".indexOf(s.charAt(i)) >= 0) i++;
    }

    private Object value() {
        if (i >= s.length()) throw err("eof");
        char c = s.charAt(i);
        if (c == '{') {
            Map<String, Object> m = new LinkedHashMap<>();
            i++;
            ws();
            if (peek('}')) return m;
            while (true) {
                ws();
                String k = str();
                ws();
                expect(':');
                ws();
                m.put(k, value());
                ws();
                if (peek('}')) return m;
                expect(',');
            }
        }
        if (c == '[') {
            List<Object> l = new ArrayList<>();
            i++;
            ws();
            if (peek(']')) return l;
            while (true) {
                ws();
                l.add(value());
                ws();
                if (peek(']')) return l;
                expect(',');
            }
        }
        if (c == '"') return str();
        if (s.startsWith("true", i)) { i += 4; return Boolean.TRUE; }
        if (s.startsWith("false", i)) { i += 5; return Boolean.FALSE; }
        if (s.startsWith("null", i)) { i += 4; return null; }
        int st = i;
        while (i < s.length() && "+-0123456789.eE".indexOf(s.charAt(i)) >= 0) i++;
        if (st == i) throw err("unexpected '" + c + "'");
        return Double.parseDouble(s.substring(st, i));
    }

    private boolean peek(char c) {
        if (i < s.length() && s.charAt(i) == c) {
            i++;
            return true;
        }
        return false;
    }

    private void expect(char c) {
        if (!peek(c)) throw err("expected '" + c + "'");
    }

    private String str() {
        expect('"');
        StringBuilder b = new StringBuilder();
        while (true) {
            if (i >= s.length()) throw err("unterminated string");
            char c = s.charAt(i++);
            if (c == '"') return b.toString();
            if (c != '\\') {
                b.append(c);
                continue;
            }
            char e = s.charAt(i++);
            switch (e) {
                case 'n': b.append('\n'); break;
                case 't': b.append('\t'); break;
                case 'r': b.append('\r'); break;
                case 'b': b.append('\b'); break;
                case 'f': b.append('\f'); break;
                case 'u': b.append((char) Integer.parseInt(s.substring(i, i + 4), 16)); i += 4; break;
                default: b.append(e);
            }
        }
    }

    // typed getters
    @SuppressWarnings("unchecked")
    static Map<String, Object> obj(Object o) {
        if (!(o instanceof Map)) throw new IllegalArgumentException("json: not an object");
        return (Map<String, Object>) o;
    }

    @SuppressWarnings("unchecked")
    static List<Object> arr(Object o) {
        if (!(o instanceof List)) throw new IllegalArgumentException("json: not an array");
        return (List<Object>) o;
    }

    static double num(Map<String, Object> m, String k) {
        Object v = m.get(k);
        if (!(v instanceof Double)) throw new IllegalArgumentException("json: '" + k + "' is not a number");
        return (Double) v;
    }

    static int optInt(Map<String, Object> m, String k, int d) {
        Object v = m.get(k);
        return v instanceof Double ? (int) (double) (Double) v : d;
    }

    static boolean optBool(Map<String, Object> m, String k, boolean d) {
        Object v = m.get(k);
        return v instanceof Boolean ? (Boolean) v : d;
    }

    static String str(Map<String, Object> m, String k) {
        Object v = m.get(k);
        if (!(v instanceof String)) throw new IllegalArgumentException("json: '" + k + "' is not a string");
        return (String) v;
    }

    static String optStr(Map<String, Object> m, String k) {
        Object v = m.get(k);
        return v instanceof String ? (String) v : null;
    }
}

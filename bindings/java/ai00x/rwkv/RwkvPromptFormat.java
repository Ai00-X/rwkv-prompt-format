package ai00x.rwkv;

import com.sun.jna.Library;
import com.sun.jna.Native;
import com.sun.jna.Pointer;

import java.nio.charset.StandardCharsets;
import java.util.Arrays;

/**
 * RWKV G1/G1x prompt format checker and auto-fixer (JNA binding).
 *
 * Requires JNA on the classpath (e.g. {@code com.sun.jna:jna:5.14.0}).
 * The native library is resolved by JNA from {@code jna.library.path}, or set
 * {@code -Djna.library.path=/path/to/rwkv-prompt-format/target/release}.
 *
 * <pre>
 *   String reportJson = RwkvPromptFormat.checkJson(text);   // parse with Jackson/Gson
 *   String fixed = RwkvPromptFormat.fixText(text, RwkvPromptFormat.FIX_ALL);
 * </pre>
 *
 * Raw JSON is returned on purpose: the binding has no JSON dependency.
 */
public final class RwkvPromptFormat {

    public static final int FIX_SAFE = 0x1;
    public static final int FIX_STRUCTURAL = 0x2;
    public static final int FIX_STRIP_METADATA = 0x4;
    public static final int FIX_ALL = FIX_SAFE | FIX_STRUCTURAL;

    /** Minimal JNA view of the C ABI. Strings are passed as explicit UTF-8 bytes. */
    public interface Lib extends Library {
        Lib INSTANCE = Native.load("rwkv_prompt_format", Lib.class);

        Pointer rwkv_pf_version();

        Pointer rwkv_pf_check(byte[] textUtf8);

        Pointer rwkv_pf_fix(byte[] textUtf8, int flags);

        Pointer rwkv_pf_fix_text(byte[] textUtf8, int flags);

        void rwkv_pf_free_string(Pointer ptr);
    }

    private static final Lib LIB = Lib.INSTANCE;

    private RwkvPromptFormat() {
    }

    /** Library version, e.g. "0.1.0". The pointer is static memory: do not free. */
    public static String version() {
        Pointer ptr = LIB.rwkv_pf_version();
        return ptr == null ? null : ptr.getString(0, "UTF-8");
    }

    /** Check a prompt, returning the raw FormatReport JSON. */
    public static String checkJson(String text) {
        return read(LIB.rwkv_pf_check(utf8z(text)));
    }

    /** Fix a prompt, returning the raw FixResult JSON (it contains "text"). */
    public static String fixJson(String text, int flags) {
        return read(LIB.rwkv_pf_fix(utf8z(text), flags));
    }

    /** Fix a prompt and return only the fixed text. Returns null on failure. */
    public static String fixText(String text, int flags) {
        Pointer ptr = LIB.rwkv_pf_fix_text(utf8z(text), flags);
        if (ptr == null) {
            return null;
        }
        try {
            return ptr.getString(0, "UTF-8");
        } finally {
            LIB.rwkv_pf_free_string(ptr);
        }
    }

    /**
     * Parsed report is intentionally not modeled: bring your own JSON parser
     * (Jackson, Gson, ...) and deserialize {@link #checkJson(String)}.
     */

    private static byte[] utf8z(String text) {
        byte[] raw = text.getBytes(StandardCharsets.UTF_8);
        byte[] z = Arrays.copyOf(raw, raw.length + 1);
        z[raw.length] = 0;
        return z;
    }

    private static String read(Pointer ptr) {
        if (ptr == null) {
            return null;
        }
        try {
            return ptr.getString(0, "UTF-8");
        } finally {
            LIB.rwkv_pf_free_string(ptr);
        }
    }
}

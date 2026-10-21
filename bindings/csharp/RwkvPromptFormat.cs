using System;
using System.Runtime.InteropServices;

namespace Ai00x.Rwkv;

/// <summary>
/// RWKV G1/G1x prompt format checker and auto-fixer (P/Invoke binding).
///
/// Drop the native library next to the executable (or add its directory to
/// PATH), or point the resolver at it:
/// <code>
///   NativeLibrary.SetDllImportResolver(...); // or copy rwkv_prompt_format.dll
/// </code>
///
/// All methods throw <see cref="InvalidOperationException"/> when the native
/// library reports an error object or returns NULL.
/// </summary>
public static class RwkvPromptFormat
{
    public const uint FixSafe = 0x1;
    public const uint FixStructural = 0x2;
    public const uint FixStripMetadata = 0x4;
    public const uint FixAll = FixSafe | FixStructural;

    private const string Lib = "rwkv_prompt_format";

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr rwkv_pf_version();

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr rwkv_pf_check([MarshalAs(UnmanagedType.LPUTF8Str)] string textUtf8);

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr rwkv_pf_fix([MarshalAs(UnmanagedType.LPUTF8Str)] string textUtf8, uint flags);

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr rwkv_pf_fix_text([MarshalAs(UnmanagedType.LPUTF8Str)] string textUtf8, uint flags);

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)]
    private static extern void rwkv_pf_free_string(IntPtr ptr);

    /// <summary>Library version, e.g. "0.1.0". The pointer is static memory: do not free.</summary>
    public static string Version() => Marshal.PtrToStringUTF8(rwkv_pf_version()) ?? string.Empty;

    /// <summary>Check a prompt; returns the FormatReport JSON.</summary>
    public static string CheckJson(string text) => Guard(Take(rwkv_pf_check(text)));

    /// <summary>Fix a prompt; returns the FixResult JSON (it contains "text").</summary>
    public static string FixJson(string text, uint flags = FixAll) => Guard(Take(rwkv_pf_fix(text, flags)));

    /// <summary>Fix a prompt and return only the fixed text.</summary>
    public static string FixText(string text, uint flags = FixAll)
    {
        var ptr = rwkv_pf_fix_text(text, flags);
        if (ptr == IntPtr.Zero)
        {
            throw new InvalidOperationException("rwkv_pf_fix_text failed (NULL input, invalid UTF-8 or panic)");
        }
        return Take(ptr);
    }

    private static string Take(IntPtr ptr)
    {
        if (ptr == IntPtr.Zero)
        {
            throw new InvalidOperationException("native call returned NULL");
        }
        try
        {
            return Marshal.PtrToStringUTF8(ptr) ?? string.Empty;
        }
        finally
        {
            rwkv_pf_free_string(ptr);
        }
    }

    private static string Guard(string json)
    {
        if (json.StartsWith("{\"error\":", StringComparison.Ordinal))
        {
            throw new InvalidOperationException(json);
        }
        return json;
    }
}

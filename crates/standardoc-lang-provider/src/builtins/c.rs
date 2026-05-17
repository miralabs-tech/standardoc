use standardoc_ir::{BuiltinEntry, BuiltinRegistry, BuiltinTag, BuiltinTier, Kind, Language};

pub(crate) fn register_all(reg: &mut BuiltinRegistry) {
    let add = |reg: &mut BuiltinRegistry,
               names: &[&str],
               kind: Kind,
               tag: BuiltinTag,
               tier: BuiltinTier| {
        for name in names {
            reg.register(BuiltinEntry::new(
                *name,
                Language::C,
                kind,
                tag.clone(),
                tier,
            ));
        }
    };

    // Standard library headers — emitted as `Kind::Module` so a
    // `#include <stdio.h>` edge resolves to a real symbol row. `stdio`
    // (no `.h`) is the canonical builtin name; the include emitter strips
    // the `.h` extension before lookup.
    add(
        reg,
        &[
            "stdio",
            "stdlib",
            "string",
            "stdint",
            "stdbool",
            "stddef",
            "stdarg",
            "math",
            "assert",
            "ctype",
            "errno",
            "time",
            "signal",
            "setjmp",
            "locale",
            "limits",
            "float",
            "inttypes",
            "wchar",
            "wctype",
            "iso646",
            "complex",
            "fenv",
            "tgmath",
            "threads",
            "stdatomic",
            "stdnoreturn",
            "stdalign",
        ],
        Kind::Module,
        BuiltinTag::Reflection,
        BuiltinTier::Edge,
    );

    // --- <stdio.h> ---
    add(
        reg,
        &[
            "printf",
            "fprintf",
            "sprintf",
            "snprintf",
            "vprintf",
            "vfprintf",
            "vsprintf",
            "vsnprintf",
            "scanf",
            "fscanf",
            "sscanf",
            "puts",
            "fputs",
            "fgets",
            "gets",
            "putchar",
            "fputc",
            "getchar",
            "fgetc",
            "putc",
            "getc",
            "perror",
        ],
        Kind::Function,
        BuiltinTag::Console,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &[
            "fopen", "freopen", "fclose", "fread", "fwrite", "fseek", "ftell", "rewind", "fflush",
            "feof", "ferror", "clearerr", "remove", "rename", "tmpfile", "tmpnam", "setbuf",
            "setvbuf", "fgetpos", "fsetpos",
        ],
        Kind::Function,
        BuiltinTag::FileSystem,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &[
            "FILE", "fpos_t", "stdin", "stdout", "stderr", "EOF", "BUFSIZ", "NULL",
        ],
        Kind::Value,
        BuiltinTag::FileSystem,
        BuiltinTier::Edge,
    );

    // --- <stdlib.h> ---
    add(
        reg,
        &["malloc", "calloc", "realloc", "free", "aligned_alloc"],
        Kind::Function,
        BuiltinTag::Memory,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &[
            "abort",
            "exit",
            "_Exit",
            "atexit",
            "at_quick_exit",
            "quick_exit",
            "system",
            "getenv",
        ],
        Kind::Function,
        BuiltinTag::Process,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &[
            "atoi", "atol", "atoll", "atof", "strtol", "strtoll", "strtoul", "strtoull", "strtod",
            "strtof", "strtold",
        ],
        Kind::Function,
        BuiltinTag::Decode,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["qsort", "bsearch"],
        Kind::Function,
        BuiltinTag::Custom { tag: "sort".into() },
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["rand", "srand"],
        Kind::Function,
        BuiltinTag::Custom {
            tag: "random".into(),
        },
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["abs", "labs", "llabs", "div", "ldiv", "lldiv"],
        Kind::Function,
        BuiltinTag::Math,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["EXIT_SUCCESS", "EXIT_FAILURE", "RAND_MAX", "MB_CUR_MAX"],
        Kind::Value,
        BuiltinTag::Process,
        BuiltinTier::Edge,
    );

    // --- <string.h> ---
    add(
        reg,
        &[
            "strlen", "strcpy", "strncpy", "strcat", "strncat", "strcmp", "strncmp", "strchr",
            "strrchr", "strstr", "strpbrk", "strspn", "strcspn", "strtok", "strtok_r", "strdup",
            "strndup", "strerror", "strxfrm", "strcoll",
        ],
        Kind::Function,
        BuiltinTag::Custom {
            tag: "string".into(),
        },
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["memcpy", "memmove", "memcmp", "memset", "memchr"],
        Kind::Function,
        BuiltinTag::Memory,
        BuiltinTier::Edge,
    );

    // --- <math.h> ---
    add(
        reg,
        &[
            "sqrt", "cbrt", "pow", "exp", "exp2", "expm1", "log", "log2", "log10", "log1p", "sin",
            "cos", "tan", "asin", "acos", "atan", "atan2", "sinh", "cosh", "tanh", "asinh",
            "acosh", "atanh", "ceil", "floor", "round", "trunc", "fabs", "fmod", "fmax", "fmin",
            "fma", "hypot", "copysign", "nan", "isnan", "isinf", "isfinite", "signbit",
        ],
        Kind::Function,
        BuiltinTag::Math,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &[
            "M_PI",
            "M_E",
            "M_SQRT2",
            "INFINITY",
            "NAN",
            "HUGE_VAL",
            "HUGE_VALF",
        ],
        Kind::Value,
        BuiltinTag::Math,
        BuiltinTier::Edge,
    );

    // --- <time.h> ---
    add(
        reg,
        &[
            "time",
            "clock",
            "difftime",
            "mktime",
            "gmtime",
            "localtime",
            "asctime",
            "ctime",
            "strftime",
            "nanosleep",
        ],
        Kind::Function,
        BuiltinTag::Time,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &[
            "time_t",
            "clock_t",
            "struct tm",
            "struct timespec",
            "CLOCKS_PER_SEC",
        ],
        Kind::Type,
        BuiltinTag::Time,
        BuiltinTier::Edge,
    );

    // --- <ctype.h> ---
    add(
        reg,
        &[
            "isalpha", "isdigit", "isalnum", "isspace", "isupper", "islower", "isprint", "isgraph",
            "ispunct", "iscntrl", "isxdigit", "toupper", "tolower",
        ],
        Kind::Function,
        BuiltinTag::Custom {
            tag: "ctype".into(),
        },
        BuiltinTier::Edge,
    );

    // --- <assert.h> ---
    add(
        reg,
        &["assert", "static_assert"],
        Kind::Macro,
        BuiltinTag::Custom {
            tag: "assert".into(),
        },
        BuiltinTier::Edge,
    );

    // --- <errno.h> ---
    add(
        reg,
        &["errno"],
        Kind::Value,
        BuiltinTag::Custom {
            tag: "errno".into(),
        },
        BuiltinTier::Edge,
    );

    // --- <signal.h> ---
    add(
        reg,
        &["signal", "raise"],
        Kind::Function,
        BuiltinTag::Process,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &[
            "SIGINT", "SIGTERM", "SIGSEGV", "SIGABRT", "SIGFPE", "SIGILL", "SIG_DFL", "SIG_IGN",
        ],
        Kind::Value,
        BuiltinTag::Process,
        BuiltinTier::Edge,
    );

    // --- <stdint.h> / <stddef.h> primitives — Drop tier ---
    // These are aliases for built-in scalar types; consumers care about
    // the value they wrap, not the typedef-wrapper itself.
    add(
        reg,
        &[
            "int8_t",
            "int16_t",
            "int32_t",
            "int64_t",
            "uint8_t",
            "uint16_t",
            "uint32_t",
            "uint64_t",
            "intptr_t",
            "uintptr_t",
            "intmax_t",
            "uintmax_t",
            "ptrdiff_t",
            "size_t",
            "ssize_t",
            "wchar_t",
            "char16_t",
            "char32_t",
            "off_t",
        ],
        Kind::Type,
        BuiltinTag::Memory,
        BuiltinTier::Drop,
    );

    // --- <stdbool.h> — primitive bool ---
    add(
        reg,
        &["bool"],
        Kind::Type,
        BuiltinTag::Memory,
        BuiltinTier::Drop,
    );
    add(
        reg,
        &["true", "false"],
        Kind::Value,
        BuiltinTag::Memory,
        BuiltinTier::Drop,
    );
}

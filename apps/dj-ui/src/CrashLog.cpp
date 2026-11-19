#include "CrashLog.h"

#include "BridgeClient.h"

#include <cstdio>
#include <cstdlib>
#include <csignal>

#ifdef Q_OS_WIN
#include <windows.h>
#include <dbghelp.h>
#else
#include <unistd.h>
#endif

namespace {

QString crashFile()
{
    return uiLogPath() + QStringLiteral(".crash");
}

// The handler writes with stdio and unbuffered on purpose: a report that is lost in a buffer is no report. This is
// not async-signal-safe in the strict sense, and it is the trade the situation asks for — the alternative is a
// journal that stops in the middle of a sentence.
void note(const char *text)
{
    const QByteArray path = crashFile().toLocal8Bit();
    std::FILE *file = std::fopen(path.constData(), "a");
    if (file == nullptr) {
        return;
    }
    std::fputs(text, file);
    std::fputc('\n', file);
    std::fflush(file);
    std::fclose(file);
}

void noteFrame(const char *prefix, const void *frame)
{
    char line[512];
#ifdef Q_OS_WIN
    // dbghelp reads the module table, not our symbols: the module and the offset inside it are what a build can be
    // matched against, and the offset is the same on every machine for one build.
    DWORD64 base = 0;
    char name[MAX_PATH] = {0};
    HMODULE module = nullptr;
    if (GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           static_cast<LPCSTR>(frame), &module)) {
        base = reinterpret_cast<DWORD64>(module);
        GetModuleFileNameA(module, name, MAX_PATH);
        const char *leaf = std::strrchr(name, '\\');
        if (leaf != nullptr) {
            std::snprintf(line, sizeof line, "%s %s+0x%llx", prefix, leaf + 1,
                          static_cast<unsigned long long>(reinterpret_cast<DWORD64>(frame) - base));
            note(line);
            return;
        }
    }
    std::snprintf(line, sizeof line, "%s %p (module unknown)", prefix, frame);
#else
    std::snprintf(line, sizeof line, "%s %p", prefix, frame);
#endif
    note(line);
}

void writeFrames(const char *title)
{
    note(title);
    void *frames[32] = {0};
#ifdef Q_OS_WIN
    const USHORT count = CaptureStackBackTrace(0, 32, frames, nullptr);
#else
    const int count = 0;
#endif
    for (int i = 0; i < static_cast<int>(count); ++i) {
        char prefix[24];
        std::snprintf(prefix, sizeof prefix, "#%d", i);
        noteFrame(prefix, frames[i]);
    }
}

#ifdef Q_OS_WIN
LONG WINAPI onUnhandled(EXCEPTION_POINTERS *info)
{
    char line[256];
    const DWORD code = info != nullptr && info->ExceptionRecord != nullptr ? info->ExceptionRecord->ExceptionCode : 0;
    const void *where = info != nullptr && info->ExceptionRecord != nullptr
                            ? info->ExceptionRecord->ExceptionAddress
                            : nullptr;
    std::snprintf(line, sizeof line, "fault: exception 0x%08lx at %p", static_cast<unsigned long>(code), where);
    note("--- the shell stopped on a fault ---");
    note(line);
    if (code == EXCEPTION_STACK_OVERFLOW) {
        note("the stack overflowed: a recursion that never returned");
    }
    writeFrames("frames:");
    return EXCEPTION_EXECUTE_HANDLER;
}
#endif

void onTerminate()
{
    note("--- the shell called std::terminate ---");
    writeFrames("frames:");
    std::_Exit(3);
}

void onSignal(int number)
{
    char line[64];
    std::snprintf(line, sizeof line, "--- the shell got signal %d ---", number);
    note(line);
    writeFrames("frames:");
    std::_Exit(4);
}

} // namespace

void installCrashLog()
{
#ifdef Q_OS_WIN
    SetUnhandledExceptionFilter(onUnhandled);
#endif
    std::set_terminate(onTerminate);
    std::signal(SIGSEGV, onSignal);
    std::signal(SIGABRT, onSignal);
    std::signal(SIGFPE, onSignal);
    std::signal(SIGILL, onSignal);
    note("== the shell started");
}

void noteShutdown()
{
    note("== the shell closed in the ordinary way");
}

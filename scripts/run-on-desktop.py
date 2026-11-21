"""Runs a command on a Windows desktop of its own, so that its windows are
never on the user's screen and cannot take the keyboard focus from them
(`npm run smoke:hidden`: the smoke test drives the app's real windows, and
on the user's own desktop it takes over their screen, while their typing
and clicking makes it fail at random steps).

    py scripts/run-on-desktop.py <desktop name> <log file> <command line...>

The command's output goes to the log file, which is printed when the
command has ended; the command's exit code is this script's. The desktop is
made in the user's own window station (WinSta0): the clipboard, the
Credential Manager and the Windows account are still the user's, so a copy
made by the user while a clipboard step runs can fail that step. No rights
beyond the user's own are needed. Windows only.
"""
import ctypes
import subprocess
import sys
from ctypes import wintypes

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

GENERIC_ALL = 0x10000000
CREATE_NO_WINDOW = 0x08000000
INFINITE = 0xFFFFFFFF


class STARTUPINFO(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("lpReserved", wintypes.LPWSTR),
        ("lpDesktop", wintypes.LPWSTR),
        ("lpTitle", wintypes.LPWSTR),
        ("dwX", wintypes.DWORD),
        ("dwY", wintypes.DWORD),
        ("dwXSize", wintypes.DWORD),
        ("dwYSize", wintypes.DWORD),
        ("dwXCountChars", wintypes.DWORD),
        ("dwYCountChars", wintypes.DWORD),
        ("dwFillAttribute", wintypes.DWORD),
        ("dwFlags", wintypes.DWORD),
        ("wShowWindow", wintypes.WORD),
        ("cbReserved2", wintypes.WORD),
        ("lpReserved2", ctypes.c_void_p),
        ("hStdInput", wintypes.HANDLE),
        ("hStdOutput", wintypes.HANDLE),
        ("hStdError", wintypes.HANDLE),
    ]


class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("hProcess", wintypes.HANDLE),
        ("hThread", wintypes.HANDLE),
        ("dwProcessId", wintypes.DWORD),
        ("dwThreadId", wintypes.DWORD),
    ]


user32.CreateDesktopW.restype = wintypes.HANDLE
user32.CreateDesktopW.argtypes = [
    wintypes.LPCWSTR, wintypes.LPCWSTR, ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p,
]
user32.CloseDesktop.argtypes = [wintypes.HANDLE]
kernel32.CreateProcessW.argtypes = [
    wintypes.LPCWSTR, wintypes.LPWSTR, ctypes.c_void_p, ctypes.c_void_p, wintypes.BOOL, wintypes.DWORD,
    ctypes.c_void_p, wintypes.LPCWSTR, ctypes.POINTER(STARTUPINFO), ctypes.POINTER(PROCESS_INFORMATION),
]
kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
kernel32.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
kernel32.CloseHandle.argtypes = [wintypes.HANDLE]


def main() -> int:
    name, log = sys.argv[1], sys.argv[2]
    command = subprocess.list2cmdline(sys.argv[3:])
    desktop = user32.CreateDesktopW(name, None, None, 0, GENERIC_ALL, None)
    if not desktop:
        print("CreateDesktop failed:", ctypes.WinError(ctypes.get_last_error()))
        return 2
    startup = STARTUPINFO()
    startup.cb = ctypes.sizeof(STARTUPINFO)
    startup.lpDesktop = "WinSta0\\" + name
    made = PROCESS_INFORMATION()
    # cmd.exe redirects the output to the log; it and everything it starts
    # are on the new desktop.
    line = ctypes.create_unicode_buffer(f'cmd.exe /d /s /c "{command} > "{log}" 2>&1"')
    ok = kernel32.CreateProcessW(
        None, line, None, None, False, CREATE_NO_WINDOW, None, None, ctypes.byref(startup), ctypes.byref(made)
    )
    if not ok:
        print("CreateProcess failed:", ctypes.WinError(ctypes.get_last_error()))
        user32.CloseDesktop(desktop)
        return 2
    kernel32.WaitForSingleObject(made.hProcess, INFINITE)
    code = wintypes.DWORD()
    kernel32.GetExitCodeProcess(made.hProcess, ctypes.byref(code))
    kernel32.CloseHandle(made.hThread)
    kernel32.CloseHandle(made.hProcess)
    user32.CloseDesktop(desktop)
    try:
        with open(log, encoding="utf-8", errors="replace") as written:
            sys.stdout.write(written.read())
    except OSError as error:
        print(f"the log {log} could not be read: {error}")
    return int(code.value)


if __name__ == "__main__":
    sys.exit(main())

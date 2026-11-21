# Watches the network endpoints of the app's processes for the smoke test
# (scripts/connections.mjs starts it; see the testing notes).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/watch-connections.ps1 -Root <pid>
#
# The processes watched: every netterm.exe and netterm-cli.exe that process
# <Root> (the smoke test's node) starts, and everything they start in turn
# (WebView2's msedgewebview2.exe, the Python the app runs, the netterm-cli
# a script runs), followed by parent process id and start time. Every
# <IntervalMs> it reads the system's TCP tables (IPv4 and IPv6) and UDP
# tables with their owning process ids (iphlpapi's GetExtendedTcpTable /
# GetExtendedUdpTable, which need no administrator rights) and prints one
# JSON line for each endpoint of a watched process not printed before:
#
#   {"kind":"tcp","pid":..,"name":..,"state":..,"local":..,"remote":..,
#    "remotePid":..,"remoteName":..,"remoteWatched":..,"remoteTest":..}
#   {"kind":"udp","pid":..,"name":..,"local":..}
#   {"kind":"process","pid":..,"parent":..,"name":..}
#
# For a loopback connection the remote end's owner is looked up (the
# listening socket, or the other end's row): remoteWatched says it is one of
# the watched processes, remoteTest that it is <Root> or another of its
# descendants (the test's own servers). It prints {"kind":"ready"} once
# the first sample is taken, and ends when <Root> is gone.
#
# What it cannot see: a connection opened and closed between two samples,
# where a datagram goes (UDP shows only the local end), and name lookups
# (on Windows they go through the DNS Client service, not the process).

param(
  [Parameter(Mandatory = $true)][int]$Root,
  [int]$IntervalMs = 50
)

$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class ConnectionWatcher
{
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct PROCESSENTRY32W
    {
        public uint dwSize;
        public uint cntUsage;
        public uint th32ProcessID;
        public IntPtr th32DefaultHeapID;
        public uint th32ModuleID;
        public uint cntThreads;
        public uint th32ParentProcessID;
        public int pcPriClassBase;
        public uint dwFlags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)]
        public string szExeFile;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint pid);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    static extern bool Process32FirstW(IntPtr snapshot, ref PROCESSENTRY32W entry);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    static extern bool Process32NextW(IntPtr snapshot, ref PROCESSENTRY32W entry);
    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll")]
    static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll")]
    static extern bool GetProcessTimes(IntPtr process, out long created, out long exited, out long kernel, out long user);
    [DllImport("iphlpapi.dll")]
    static extern uint GetExtendedTcpTable(IntPtr table, ref int size, bool sorted, int family, int tableClass, uint reserved);
    [DllImport("iphlpapi.dll")]
    static extern uint GetExtendedUdpTable(IntPtr table, ref int size, bool sorted, int family, int tableClass, uint reserved);

    const int AF_INET = 2;
    const int AF_INET6 = 23;
    const int TCP_TABLE_OWNER_PID_ALL = 5;
    const int UDP_TABLE_OWNER_PID = 1;
    static readonly IntPtr INVALID_HANDLE = new IntPtr(-1);

    class Proc { public uint Pid; public uint Parent; public string Name; public long Created; }

    class Row
    {
        public bool Tcp;
        public string State;
        public string LocalAddress; public int LocalPort;
        public string RemoteAddress; public int RemotePort;
        public uint Pid;
        public string Local { get { return Endpoint(LocalAddress, LocalPort); } }
        public string Remote { get { return Endpoint(RemoteAddress, RemotePort); } }
    }

    static readonly string[] States = { "", "CLOSED", "LISTEN", "SYN_SENT", "SYN_RCVD", "ESTABLISHED",
        "FIN_WAIT1", "FIN_WAIT2", "CLOSE_WAIT", "CLOSING", "LAST_ACK", "TIME_WAIT", "DELETE_TCB" };

    static string Endpoint(string address, int port)
    {
        return address.Contains(":") ? "[" + address + "]:" + port : address + ":" + port;
    }

    static long CreationTime(uint pid)
    {
        IntPtr handle = OpenProcess(0x1000, false, pid);
        if (handle == IntPtr.Zero) return 0;
        long created, exited, kernel, user;
        bool ok = GetProcessTimes(handle, out created, out exited, out kernel, out user);
        CloseHandle(handle);
        return ok ? created : 0;
    }

    static Dictionary<uint, Proc> Processes()
    {
        var found = new Dictionary<uint, Proc>();
        IntPtr snapshot = CreateToolhelp32Snapshot(2, 0);
        if (snapshot == INVALID_HANDLE) return found;
        var entry = new PROCESSENTRY32W();
        entry.dwSize = (uint)Marshal.SizeOf(typeof(PROCESSENTRY32W));
        if (Process32FirstW(snapshot, ref entry))
        {
            do
            {
                found[entry.th32ProcessID] = new Proc { Pid = entry.th32ProcessID, Parent = entry.th32ParentProcessID, Name = entry.szExeFile };
            } while (Process32NextW(snapshot, ref entry));
        }
        CloseHandle(snapshot);
        return found;
    }

    static byte[] Table(bool tcp, int family)
    {
        int size = 0;
        for (int attempt = 0; attempt < 5; attempt++)
        {
            IntPtr buffer = size > 0 ? Marshal.AllocHGlobal(size) : IntPtr.Zero;
            try
            {
                uint result = tcp
                    ? GetExtendedTcpTable(buffer, ref size, false, family, TCP_TABLE_OWNER_PID_ALL, 0)
                    : GetExtendedUdpTable(buffer, ref size, false, family, UDP_TABLE_OWNER_PID, 0);
                if (result == 0 && buffer != IntPtr.Zero)
                {
                    var bytes = new byte[size];
                    Marshal.Copy(buffer, bytes, 0, size);
                    return bytes;
                }
                if (result != 122) return new byte[0]; // not ERROR_INSUFFICIENT_BUFFER
            }
            finally
            {
                if (buffer != IntPtr.Zero) Marshal.FreeHGlobal(buffer);
            }
        }
        return new byte[0];
    }

    static int Port(byte[] b, int at) { return (b[at] << 8) | b[at + 1]; }

    static string V4(byte[] b, int at) { return b[at] + "." + b[at + 1] + "." + b[at + 2] + "." + b[at + 3]; }

    static string V6(byte[] b, int at)
    {
        var bytes = new byte[16];
        Array.Copy(b, at, bytes, 0, 16);
        return new System.Net.IPAddress(bytes).ToString();
    }

    static List<Row> Rows()
    {
        var rows = new List<Row>();
        byte[] t = Table(true, AF_INET);
        // MIB_TCPROW_OWNER_PID: state, local addr, local port, remote addr, remote port, pid (6 DWORDs).
        if (t.Length >= 4)
        {
            int count = BitConverter.ToInt32(t, 0);
            for (int i = 0; i < count; i++)
            {
                int at = 4 + i * 24;
                if (at + 24 > t.Length) break;
                int state = BitConverter.ToInt32(t, at);
                rows.Add(new Row
                {
                    Tcp = true,
                    State = state >= 0 && state < States.Length ? States[state] : state.ToString(),
                    LocalAddress = V4(t, at + 4), LocalPort = Port(t, at + 8),
                    RemoteAddress = V4(t, at + 12), RemotePort = Port(t, at + 16),
                    Pid = BitConverter.ToUInt32(t, at + 20),
                });
            }
        }
        byte[] t6 = Table(true, AF_INET6);
        // MIB_TCP6ROW_OWNER_PID: local addr (16), scope, port, remote addr (16), scope, port, state, pid.
        if (t6.Length >= 4)
        {
            int count = BitConverter.ToInt32(t6, 0);
            for (int i = 0; i < count; i++)
            {
                int at = 4 + i * 56;
                if (at + 56 > t6.Length) break;
                int state = BitConverter.ToInt32(t6, at + 48);
                rows.Add(new Row
                {
                    Tcp = true,
                    State = state >= 0 && state < States.Length ? States[state] : state.ToString(),
                    LocalAddress = V6(t6, at), LocalPort = Port(t6, at + 20),
                    RemoteAddress = V6(t6, at + 24), RemotePort = Port(t6, at + 44),
                    Pid = BitConverter.ToUInt32(t6, at + 52),
                });
            }
        }
        byte[] u = Table(false, AF_INET);
        // MIB_UDPROW_OWNER_PID: local addr, local port, pid.
        if (u.Length >= 4)
        {
            int count = BitConverter.ToInt32(u, 0);
            for (int i = 0; i < count; i++)
            {
                int at = 4 + i * 12;
                if (at + 12 > u.Length) break;
                rows.Add(new Row
                {
                    LocalAddress = V4(u, at), LocalPort = Port(u, at + 4),
                    RemoteAddress = "", Pid = BitConverter.ToUInt32(u, at + 8),
                });
            }
        }
        byte[] u6 = Table(false, AF_INET6);
        // MIB_UDP6ROW_OWNER_PID: local addr (16), scope, port, pid.
        if (u6.Length >= 4)
        {
            int count = BitConverter.ToInt32(u6, 0);
            for (int i = 0; i < count; i++)
            {
                int at = 4 + i * 28;
                if (at + 28 > u6.Length) break;
                rows.Add(new Row
                {
                    LocalAddress = V6(u6, at), LocalPort = Port(u6, at + 20),
                    RemoteAddress = "", Pid = BitConverter.ToUInt32(u6, at + 24),
                });
            }
        }
        return rows;
    }

    static string Quote(string text)
    {
        var s = new StringBuilder("\"");
        foreach (char c in text ?? "")
        {
            if (c == '"' || c == '\\') s.Append('\\').Append(c);
            else if (c < ' ') s.Append("\\u").Append(((int)c).ToString("x4"));
            else s.Append(c);
        }
        return s.Append('"').ToString();
    }

    static bool IsLoopback(string address)
    {
        return address.StartsWith("127.") || address == "::1" || address.StartsWith("::ffff:127.");
    }

    public static void Run(int root, int intervalMs)
    {
        uint rootPid = (uint)root;
        var watched = new Dictionary<uint, Proc>();
        var seen = new HashSet<string>();
        var output = Console.Out;
        bool ready = false;
        while (true)
        {
            var processes = Processes();
            if (!processes.ContainsKey(rootPid)) return;
            foreach (uint pid in new List<uint>(watched.Keys))
            {
                Proc now;
                if (!processes.TryGetValue(pid, out now) || now.Parent != watched[pid].Parent) watched.Remove(pid);
            }
            // The test's own processes: the root and every other descendant.
            var test = new HashSet<uint>();
            test.Add(rootPid);
            bool grew = true;
            while (grew)
            {
                grew = false;
                foreach (var p in processes.Values)
                {
                    if (watched.ContainsKey(p.Pid) || p.Pid == 0) continue;
                    bool rootApp = p.Parent == rootPid && (string.Equals(p.Name, "netterm.exe", StringComparison.OrdinalIgnoreCase) || string.Equals(p.Name, "netterm-cli.exe", StringComparison.OrdinalIgnoreCase));
                    Proc parent;
                    bool childOfWatched = watched.TryGetValue(p.Parent, out parent);
                    if (rootApp || childOfWatched)
                    {
                        long created = CreationTime(p.Pid);
                        // A parent id reused by a later process is no parent.
                        if (childOfWatched && created != 0 && parent.Created != 0 && created < parent.Created) continue;
                        // Nor is it one of a process this user cannot even
                        // ask its start time (a system process, such as
                        // wininit.exe, whose own parent ended at boot and
                        // whose parent id a process of the app got later):
                        // everything the app starts runs as this user and
                        // can be asked. Taking such a process in would take
                        // in every Windows service below it.
                        if (childOfWatched && created == 0) continue;
                        p.Created = created;
                        watched[p.Pid] = p;
                        grew = true;
                        output.WriteLine("{\"kind\":\"process\",\"pid\":" + p.Pid + ",\"parent\":" + p.Parent + ",\"name\":" + Quote(p.Name) + ",\"created\":" + created + "}");
                    }
                    else if (test.Contains(p.Parent) && !test.Contains(p.Pid))
                    {
                        test.Add(p.Pid);
                        grew = true;
                    }
                }
            }
            var rows = Rows();
            var owners = new Dictionary<string, uint>();
            foreach (var r in rows)
            {
                if (!r.Tcp || r.Pid == 0) continue;
                if (r.State == "LISTEN") owners[r.LocalPort + "/listen"] = r.Pid;
                else owners[r.Local] = r.Pid;
            }
            foreach (var r in rows)
            {
                Proc owner;
                if (!watched.TryGetValue(r.Pid, out owner)) continue;
                if (!r.Tcp)
                {
                    string ukey = "udp " + r.Pid + " " + r.Local;
                    if (seen.Add(ukey))
                        output.WriteLine("{\"kind\":\"udp\",\"pid\":" + r.Pid + ",\"name\":" + Quote(owner.Name) + ",\"local\":" + Quote(r.Local) + ",\"localAddress\":" + Quote(r.LocalAddress) + "}");
                    continue;
                }
                string key = "tcp " + r.Pid + " " + r.State + " " + r.Local + " " + r.Remote;
                if (!seen.Add(key)) continue;
                uint remotePid = 0;
                if (r.State != "LISTEN" && IsLoopback(r.RemoteAddress))
                {
                    if (!owners.TryGetValue(r.Remote, out remotePid))
                        owners.TryGetValue(r.RemotePort + "/listen", out remotePid);
                }
                Proc remote;
                string remoteName = remotePid != 0 && processes.TryGetValue(remotePid, out remote) ? remote.Name : "";
                output.WriteLine("{\"kind\":\"tcp\",\"pid\":" + r.Pid + ",\"name\":" + Quote(owner.Name)
                    + ",\"state\":" + Quote(r.State) + ",\"local\":" + Quote(r.Local) + ",\"remote\":" + Quote(r.Remote)
                    + ",\"remoteAddress\":" + Quote(r.RemoteAddress) + ",\"remotePort\":" + r.RemotePort
                    + ",\"localAddress\":" + Quote(r.LocalAddress)
                    + ",\"remotePid\":" + remotePid + ",\"remoteName\":" + Quote(remoteName)
                    + ",\"remoteWatched\":" + (remotePid != 0 && watched.ContainsKey(remotePid) ? "true" : "false")
                    + ",\"remoteTest\":" + (remotePid != 0 && test.Contains(remotePid) ? "true" : "false") + "}");
            }
            if (!ready)
            {
                ready = true;
                output.WriteLine("{\"kind\":\"ready\"}");
            }
            output.Flush();
            Thread.Sleep(intervalMs);
        }
    }
}
"@

[ConnectionWatcher]::Run($Root, $IntervalMs)

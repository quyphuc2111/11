import { useState, useEffect, useRef } from "react";
import "./App.css";
import io, { Socket } from "socket.io-client";

const DEFAULT_SERVER_URL = "http://localhost:3001";
const isTauri = !!(window as any).__TAURI_INTERNALS__;

const USERS = {
  admin: { password: "admin123", role: "admin" as const },
  client: { password: "client123", role: "client" as const },
};

type Role = "admin" | "client";
type ClientInfo = {
  id: string;
  ip: string;
  name: string;
  stream?: MediaStream;
  isLocked: boolean;
  isSelected: boolean;
};

function App() {
  const [isLoggedIn, setIsLoggedIn] = useState(false);
  const [role, setRole] = useState<Role | null>(null);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [loginError, setLoginError] = useState("");

  const [serverIp, setServerIp] = useState("localhost");
  const [socket, setSocket] = useState<Socket | null>(null);
  const [status, setStatus] = useState("Disconnected");
  const [myIp, setMyIp] = useState("");

  const [clients, setClients] = useState<Map<string, ClientInfo>>(new Map());
  const [selectedClient, setSelectedClient] = useState<string | null>(null);
  const [viewMode, setViewMode] = useState<"grid" | "list">("grid");
  const [thumbnailSize, setThumbnailSize] = useState<"small" | "medium" | "large">("medium");

  const [isLocked, setIsLocked] = useState(false);
  const [lockMessage, setLockMessage] = useState("");
  const [clientName, setClientName] = useState("");
  const [isSharing, setIsSharing] = useState(false);

  const localStream = useRef<MediaStream | null>(null);
  const peerConnections = useRef<Map<string, RTCPeerConnection>>(new Map());

  const handleLogin = () => {
    const user = USERS[username as keyof typeof USERS];
    if (user && user.password === password) {
      setRole(user.role);
      setIsLoggedIn(true);
      setLoginError("");
    } else {
      setLoginError("Sai tên đăng nhập hoặc mật khẩu");
    }
  };

  useEffect(() => {
    if (!isLoggedIn || role !== "admin") return;
    const startServer = async () => {
      if (!isTauri) return;
      try {
        const { Command } = await import("@tauri-apps/plugin-shell");
        await Command.sidecar("bin/server").spawn();
      } catch (e) { console.error(e); }
      try {
        const { Command } = await import("@tauri-apps/plugin-shell");
        const output = await Command.create("get-ip").execute();
        if (output.code === 0) setMyIp(output.stdout.trim());
      } catch (e) { console.error(e); }
    };
    startServer();
  }, [isLoggedIn, role]);

  useEffect(() => {
    if (!isLoggedIn || !role) return;
    const targetUrl = `http://${serverIp}:3001`;
    const newSocket = io(targetUrl, { autoConnect: false });
    setSocket(newSocket);
    return () => { newSocket.disconnect(); };
  }, [serverIp, isLoggedIn, role]);

  useEffect(() => {
    if (!socket || !role) return;

    socket.on("connect", () => {
      setStatus("Connected");
      socket.emit("register", { role, name: clientName || `PC-${Math.random().toString(36).substr(2, 4)}` });
    });

    socket.on("disconnect", () => setStatus("Disconnected"));

    if (role === "admin") {
      socket.on("client-list", (list: { id: string; ip: string; name: string }[]) => {
        setClients((prev) => {
          const newMap = new Map(prev);
          list.forEach((c) => {
            if (!newMap.has(c.id)) {
              newMap.set(c.id, { ...c, isLocked: false, isSelected: false });
            } else {
              const existing = newMap.get(c.id)!;
              newMap.set(c.id, { ...existing, ip: c.ip, name: c.name });
            }
          });
          Array.from(newMap.keys()).forEach((id) => {
            if (!list.find((c) => c.id === id)) {
              newMap.delete(id);
              peerConnections.current.get(id)?.close();
              peerConnections.current.delete(id);
            }
          });
          return newMap;
        });
      });

      socket.on("offer", async (payload: any) => {
        const pc = createPeerConnection(payload.callerId);
        await pc.setRemoteDescription(new RTCSessionDescription(payload.sdp));
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        socket.emit("answer", { target: payload.callerId, sdp: pc.localDescription });
      });

      socket.on("ice-candidate", async (payload: any) => {
        const pc = peerConnections.current.get(payload.from);
        if (pc) await pc.addIceCandidate(new RTCIceCandidate(payload.candidate));
      });
    }

    if (role === "client") {
      socket.on("answer", async (payload: any) => {
        const pc = peerConnections.current.get("admin");
        if (pc) await pc.setRemoteDescription(new RTCSessionDescription(payload.sdp));
      });
      socket.on("ice-candidate", async (payload: any) => {
        const pc = peerConnections.current.get("admin");
        if (pc) await pc.addIceCandidate(new RTCIceCandidate(payload.candidate));
      });
      socket.on("lock", (data: { message: string }) => {
        setIsLocked(true);
        setLockMessage(data.message);
      });
      socket.on("unlock", () => {
        setIsLocked(false);
        setLockMessage("");
      });
    }

    socket.connect();
    return () => { socket.removeAllListeners(); };
  }, [socket, role]);

  const createPeerConnection = (peerId: string) => {
    const pc = new RTCPeerConnection({ iceServers: [{ urls: "stun:stun.l.google.com:19302" }] });
    pc.onicecandidate = (e) => {
      if (e.candidate) socket?.emit("ice-candidate", { target: peerId, candidate: e.candidate });
    };
    if (role === "admin") {
      pc.ontrack = (e) => {
        setClients((prev) => {
          const newMap = new Map(prev);
          const client = newMap.get(peerId);
          if (client) newMap.set(peerId, { ...client, stream: e.streams[0] });
          return newMap;
        });
      };
    }
    peerConnections.current.set(peerId, pc);
    return pc;
  };

  const startScreenShare = async () => {
    try {
      const stream = await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false });
      localStream.current = stream;
      setIsSharing(true);
      setStatus("Đang chia sẻ màn hình");

      const pc = createPeerConnection("admin");
      stream.getTracks().forEach((t) => pc.addTrack(t, stream));

      // Khi user stop share từ browser
      stream.getVideoTracks()[0].onended = () => {
        setIsSharing(false);
        setStatus("Đã dừng chia sẻ");
      };

      const offer = await pc.createOffer();
      await pc.setLocalDescription(offer);
      socket?.emit("offer", { sdp: pc.localDescription });
    } catch (e: any) {
      setStatus(`Error: ${e.message}`);
      setIsSharing(false);
    }
  };

  const lockClient = (id: string) => {
    socket?.emit("lock-client", { clientId: id, message: "Màn hình đã bị khóa" });
    setClients((prev) => {
      const m = new Map(prev);
      const c = m.get(id);
      if (c) m.set(id, { ...c, isLocked: true });
      return m;
    });
  };

  const unlockClient = (id: string) => {
    socket?.emit("unlock-client", { clientId: id });
    setClients((prev) => {
      const m = new Map(prev);
      const c = m.get(id);
      if (c) m.set(id, { ...c, isLocked: false });
      return m;
    });
  };

  const lockAll = () => {
    socket?.emit("lock-all", { message: "Tất cả màn hình đã bị khóa" });
    setClients((prev) => {
      const m = new Map(prev);
      m.forEach((c, id) => m.set(id, { ...c, isLocked: true }));
      return m;
    });
  };

  const unlockAll = () => {
    socket?.emit("unlock-all");
    setClients((prev) => {
      const m = new Map(prev);
      m.forEach((c, id) => m.set(id, { ...c, isLocked: false }));
      return m;
    });
  };

  const selectClient = (id: string) => {
    setSelectedClient(selectedClient === id ? null : id);
    setClients((prev) => {
      const m = new Map(prev);
      m.forEach((c, cid) => m.set(cid, { ...c, isSelected: cid === id && selectedClient !== id }));
      return m;
    });
  };

  const getThumbnailClass = () => {
    switch (thumbnailSize) {
      case "small": return "thumb-small";
      case "large": return "thumb-large";
      default: return "thumb-medium";
    }
  };

  // Login
  if (!isLoggedIn) {
    return (
      <div className="login-page">
        <div className="login-box">
          <div className="login-logo">🖥️</div>
          <h1>Quản Lý Phòng Máy</h1>
          <input type="text" placeholder="Tên đăng nhập" value={username} onChange={(e) => setUsername(e.target.value)} />
          <input type="password" placeholder="Mật khẩu" value={password} onChange={(e) => setPassword(e.target.value)} onKeyDown={(e) => e.key === "Enter" && handleLogin()} />
          {loginError && <div className="login-error">{loginError}</div>}
          <button onClick={handleLogin}>Đăng nhập</button>
          <div className="login-hint">Admin: admin/admin123 | Client: client/client123</div>
        </div>
      </div>
    );
  }

  // Client Lock Screen
  if (role === "client" && isLocked) {
    return (
      <div className="lock-screen">
        <div className="lock-icon">🔒</div>
        <h1>{lockMessage}</h1>
        <p>Vui lòng chờ giáo viên mở khóa</p>
      </div>
    );
  }

  // Client View
  if (role === "client") {
    return (
      <div className="client-page">
        <div className="client-status">
          <div className={`status-dot ${status === "Connected" ? "online" : ""}`}></div>
          <span>{status}</span>
        </div>
        <div className="client-content">
          <div className="client-icon">{isSharing ? "📺" : "🖥️"}</div>
          {isSharing ? (
            <>
              <h2>Đang chia sẻ màn hình</h2>
              <p>Màn hình của bạn đang được giáo viên theo dõi</p>
            </>
          ) : (
            <>
              <h2>Sẵn sàng chia sẻ</h2>
              <p>Nhấn nút bên dưới để bắt đầu chia sẻ màn hình</p>
              <button className="share-btn" onClick={startScreenShare}>
                🖥️ Bắt đầu chia sẻ màn hình
              </button>
            </>
          )}
        </div>
      </div>
    );
  }

  // Admin View - NetSupport Style
  const clientsArray = Array.from(clients.values());
  const lockedCount = clientsArray.filter((c) => c.isLocked).length;

  return (
    <div className="admin-page">
      {/* Toolbar */}
      <div className="toolbar">
        <div className="toolbar-group">
          <button className="tool-btn" onClick={lockAll} title="Khóa tất cả">
            <span className="tool-icon">🔒</span>
            <span className="tool-label">Khóa tất cả</span>
          </button>
          <button className="tool-btn" onClick={unlockAll} title="Mở khóa tất cả">
            <span className="tool-icon">🔓</span>
            <span className="tool-label">Mở khóa</span>
          </button>
          <div className="toolbar-divider"></div>
          <button className="tool-btn" onClick={() => selectedClient && lockClient(selectedClient)} disabled={!selectedClient} title="Khóa máy đã chọn">
            <span className="tool-icon">🖥️🔒</span>
            <span className="tool-label">Khóa máy</span>
          </button>
          <button className="tool-btn" onClick={() => selectedClient && unlockClient(selectedClient)} disabled={!selectedClient} title="Mở khóa máy đã chọn">
            <span className="tool-icon">🖥️🔓</span>
            <span className="tool-label">Mở máy</span>
          </button>
        </div>
        <div className="toolbar-group">
          <div className="toolbar-divider"></div>
          <button className={`tool-btn ${viewMode === "grid" ? "active" : ""}`} onClick={() => setViewMode("grid")} title="Xem dạng lưới">
            <span className="tool-icon">▦</span>
          </button>
          <button className={`tool-btn ${viewMode === "list" ? "active" : ""}`} onClick={() => setViewMode("list")} title="Xem dạng danh sách">
            <span className="tool-icon">☰</span>
          </button>
          <div className="toolbar-divider"></div>
          <select value={thumbnailSize} onChange={(e) => setThumbnailSize(e.target.value as any)} className="size-select">
            <option value="small">Nhỏ</option>
            <option value="medium">Vừa</option>
            <option value="large">Lớn</option>
          </select>
        </div>
      </div>

      <div className="main-content">
        {/* Sidebar */}
        <div className="sidebar">
          <div className="sidebar-header">
            <h3>📋 Danh sách máy</h3>
            <span className="client-count">{clientsArray.length}</span>
          </div>
          <div className="sidebar-stats">
            <div className="stat">
              <span className="stat-icon online">●</span>
              <span>Online: {clientsArray.length}</span>
            </div>
            <div className="stat">
              <span className="stat-icon locked">●</span>
              <span>Đã khóa: {lockedCount}</span>
            </div>
          </div>
          <div className="client-list">
            {clientsArray.map((c) => (
              <div key={c.id} className={`client-item ${c.isSelected ? "selected" : ""} ${c.isLocked ? "locked" : ""}`} onClick={() => selectClient(c.id)}>
                <span className="client-icon-small">{c.isLocked ? "🔒" : "🖥️"}</span>
                <div className="client-info">
                  <div className="client-name">{c.name}</div>
                  <div className="client-ip">{c.ip}</div>
                </div>
                <span className={`client-status-dot ${c.stream ? "streaming" : ""}`}></span>
              </div>
            ))}
            {clientsArray.length === 0 && <div className="no-clients">Chưa có máy kết nối</div>}
          </div>
          <div className="sidebar-footer">
            <div className="server-info">
              <small>Server: {serverIp}:3001</small>
              {myIp && <small>IP: {myIp}</small>}
            </div>
          </div>
        </div>

        {/* Screen Grid */}
        <div className="screen-area">
          {viewMode === "grid" ? (
            <div className={`screen-grid ${getThumbnailClass()}`}>
              {clientsArray.map((c) => (
                <div key={c.id} className={`screen-card ${c.isSelected ? "selected" : ""} ${c.isLocked ? "locked" : ""}`} onClick={() => selectClient(c.id)}>
                  <div className="screen-header">
                    <span>{c.name}</span>
                    {c.isLocked && <span className="lock-badge">🔒</span>}
                  </div>
                  <div className="screen-view">
                    {c.stream ? (
                      <video autoPlay playsInline muted ref={(v) => { if (v && v.srcObject !== c.stream) v.srcObject = c.stream!; }} />
                    ) : (
                      <div className="no-video">
                        <span>📺</span>
                        <small>Đang kết nối...</small>
                      </div>
                    )}
                    {c.isLocked && <div className="screen-lock-overlay">🔒</div>}
                  </div>
                  <div className="screen-footer">
                    <span className="screen-ip">{c.ip}</span>
                  </div>
                </div>
              ))}
              {clientsArray.length === 0 && (
                <div className="empty-state">
                  <span>🖥️</span>
                  <h3>Chưa có máy trạm nào</h3>
                  <p>Các máy client sẽ xuất hiện ở đây khi kết nối</p>
                </div>
              )}
            </div>
          ) : (
            <div className="screen-list">
              <table>
                <thead>
                  <tr>
                    <th>Tên máy</th>
                    <th>IP</th>
                    <th>Trạng thái</th>
                    <th>Hành động</th>
                  </tr>
                </thead>
                <tbody>
                  {clientsArray.map((c) => (
                    <tr key={c.id} className={c.isSelected ? "selected" : ""} onClick={() => selectClient(c.id)}>
                      <td><span className="list-icon">{c.isLocked ? "🔒" : "🖥️"}</span> {c.name}</td>
                      <td>{c.ip}</td>
                      <td><span className={`status-badge ${c.isLocked ? "locked" : "online"}`}>{c.isLocked ? "Đã khóa" : "Online"}</span></td>
                      <td>
                        {c.isLocked ? (
                          <button className="action-btn unlock" onClick={(e) => { e.stopPropagation(); unlockClient(c.id); }}>Mở khóa</button>
                        ) : (
                          <button className="action-btn lock" onClick={(e) => { e.stopPropagation(); lockClient(c.id); }}>Khóa</button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      </div>

      {/* Status Bar */}
      <div className="statusbar">
        <div className="statusbar-left">
          <span className={`connection-status ${status === "Connected" ? "online" : ""}`}>● {status}</span>
          <span>|</span>
          <span>Máy trạm: {clientsArray.length}</span>
          <span>|</span>
          <span>Đã khóa: {lockedCount}</span>
        </div>
        <div className="statusbar-right">
          <span>Quản Lý Phòng Máy v1.0</span>
        </div>
      </div>
    </div>
  );
}

export default App;

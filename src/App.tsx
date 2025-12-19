import { useState, useEffect } from "react";
import "./App.css";
import io, { Socket } from "socket.io-client";

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
  screenData?: string; // base64 image
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
  const [debugLogs, setDebugLogs] = useState<string[]>([]);
  const [remoteControlClient, setRemoteControlClient] = useState<string | null>(null);

  const addLog = (msg: string) => {
    const time = new Date().toLocaleTimeString();
    setDebugLogs((prev) => [...prev.slice(-20), `[${time}] ${msg}`]);
  };

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

  // Admin: Start server
  useEffect(() => {
    if (!isLoggedIn || role !== "admin") return;
    const startServer = async () => {
      if (!isTauri) return;
      try {
        const { Command } = await import("@tauri-apps/plugin-shell");
        await Command.sidecar("bin/server").spawn();
      } catch (e) {
        console.error(e);
      }
      try {
        const { Command } = await import("@tauri-apps/plugin-shell");
        const output = await Command.create("get-ip").execute();
        if (output.code === 0) setMyIp(output.stdout.trim());
      } catch (e) {
        console.error(e);
      }
    };
    startServer();
  }, [isLoggedIn, role]);

  // Socket connection
  useEffect(() => {
    if (!isLoggedIn || !role) return;
    // Admin kết nối localhost vì server chạy trên máy admin
    // Client kết nối đến IP của máy admin
    const targetUrl = role === "admin" 
      ? "http://localhost:3001" 
      : `http://${serverIp}:3001`;
    console.log("Connecting to:", targetUrl);
    const newSocket = io(targetUrl, { autoConnect: false });
    setSocket(newSocket);
    return () => {
      newSocket.disconnect();
    };
  }, [serverIp, isLoggedIn, role]);

  // Client: Start screen capture
  useEffect(() => {
    if (!isLoggedIn || role !== "client" || !socket) return;

    let cleanup: (() => void) | null = null;
    let captureStarted = false;

    const startCapture = async () => {
      if (captureStarted) return;
      captureStarted = true;

      addLog(`Starting capture... isTauri: ${isTauri}`);

      if (isTauri) {
        // Thử Rust capture trước
        try {
          addLog("Importing Tauri APIs...");
          const { invoke } = await import("@tauri-apps/api/core");
          const { listen } = await import("@tauri-apps/api/event");

          addLog("Setting up event listener...");
          let frameCount = 0;
          const unlisten = await listen<string>("screen-frame", (event) => {
            frameCount++;
            if (socket?.connected) {
              socket.emit("screen-frame", event.payload);
              if (frameCount % 10 === 0) {
                addLog(`Sent ${frameCount} frames (last: ${event.payload.length} bytes)`);
              }
            } else {
              addLog(`Socket not connected! Frame #${frameCount} dropped`);
            }
          });

          addLog("Calling start_capture_loop...");
          await invoke("start_capture_loop", { intervalMs: 1000 }); // 1 FPS để giảm lag
          addLog("Capture loop started!");
          setStatus("Đang chia sẻ màn hình (Native)");

          cleanup = () => {
            unlisten();
            invoke("stop_capture_loop");
          };
          return;
        } catch (e: any) {
          addLog(`Rust capture failed: ${e.message || e}`);
        }
      }

      // Fallback: Browser getDisplayMedia (cần user click)
      addLog("Trying browser getDisplayMedia...");
      try {
        const stream = await navigator.mediaDevices.getDisplayMedia({ video: true });
        addLog("Got display media stream");
        const video = document.createElement("video");
        video.srcObject = stream;
        video.play();

        const canvas = document.createElement("canvas");
        const ctx = canvas.getContext("2d")!;

        let frameCount = 0;
        const captureFrame = () => {
          if (!stream.active) return;
          canvas.width = 800;
          canvas.height = (800 * video.videoHeight) / video.videoWidth;
          ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
          const data = canvas.toDataURL("image/jpeg", 0.6);
          socket?.emit("screen-frame", data);
          frameCount++;
          if (frameCount % 10 === 0) addLog(`Sent ${frameCount} frames`);
        };

        const interval = setInterval(captureFrame, 500);
        setStatus("Đang chia sẻ màn hình (Browser)");

        cleanup = () => {
          clearInterval(interval);
          stream.getTracks().forEach((t) => t.stop());
        };
      } catch (e: any) {
        addLog(`Browser capture failed: ${e.message}`);
        setStatus(`Lỗi: ${e.message}`);
      }
    };

    if (socket.connected) {
      startCapture();
    }
    socket.on("connect", startCapture);

    return () => {
      cleanup?.();
      socket.off("connect", startCapture);
    };
  }, [isLoggedIn, role, socket]);

  // Socket events
  useEffect(() => {
    if (!socket || !role) return;

    socket.on("connect", () => {
      addLog("Socket connected!");
      setStatus("Connected");
      const name = `PC-${Math.random().toString(36).slice(2, 6).toUpperCase()}`;
      socket.emit("register", { role, name });
    });

    socket.on("disconnect", () => {
      addLog("Socket disconnected");
      setStatus("Disconnected");
    });

    socket.on("connect_error", (err) => {
      addLog(`Connection error: ${err.message}`);
    });

    if (role === "admin") {
      socket.on("client-list", (list: { id: string; ip: string; name: string }[]) => {
        addLog(`Got client list: ${list.length} clients`);
        setClients((prev) => {
          const newMap = new Map(prev);
          list.forEach((c) => {
            if (!newMap.has(c.id)) {
              newMap.set(c.id, { ...c, isLocked: false, isSelected: false });
              addLog(`New client: ${c.name} (${c.ip})`);
            } else {
              const existing = newMap.get(c.id)!;
              newMap.set(c.id, { ...existing, ip: c.ip, name: c.name });
            }
          });
          Array.from(newMap.keys()).forEach((id) => {
            if (!list.find((c) => c.id === id)) {
              newMap.delete(id);
            }
          });
          return newMap;
        });
      });

      // Receive screen frames from clients
      socket.on("screen-frame", ({ clientId, data }: { clientId: string; data: string }) => {
        addLog(`Received frame from ${clientId}, size: ${data?.length || 0}`);
        setClients((prev) => {
          const newMap = new Map(prev);
          const client = newMap.get(clientId);
          if (client) {
            newMap.set(clientId, { ...client, screenData: data });
          } else {
            addLog(`Client ${clientId} not found in list!`);
          }
          return newMap;
        });
      });
    }

    if (role === "client") {
      socket.on("lock", async (data: { message: string }) => {
        addLog(`Received lock command: ${data.message}`);
        setIsLocked(true);
        setLockMessage(data.message);
        
        // Fullscreen + always on top
        if (isTauri) {
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            await invoke("set_lock_screen", { lock: true, message: data.message });
            addLog("Lock screen activated");
          } catch (e: any) {
            addLog(`Lock screen error: ${e.message}`);
          }
        }
      });
      
      socket.on("unlock", async () => {
        addLog("Received unlock command");
        setIsLocked(false);
        setLockMessage("");
        
        if (isTauri) {
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            await invoke("set_lock_screen", { lock: false, message: "" });
            addLog("Lock screen deactivated");
          } catch (e: any) {
            addLog(`Unlock error: ${e.message}`);
          }
        }
      });

      // Remote control handlers
      socket.on("remote-mouse-move", async ({ x, y }: { x: number; y: number }) => {
        if (isTauri) {
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            await invoke("remote_mouse_move", { x, y });
          } catch (e) {
            console.error("Mouse move error:", e);
          }
        }
      });

      socket.on("remote-mouse-click", async ({ button }: { button: string }) => {
        if (isTauri) {
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            await invoke("remote_mouse_click", { button });
          } catch (e) {
            console.error("Mouse click error:", e);
          }
        }
      });

      socket.on("remote-key-press", async ({ key }: { key: string }) => {
        if (isTauri) {
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            await invoke("remote_key_press", { key });
          } catch (e) {
            console.error("Key press error:", e);
          }
        }
      });
    }

    socket.connect();

    return () => {
      socket.removeAllListeners();
    };
  }, [socket, role]);

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

  // Remote control functions
  const startRemoteControl = (clientId: string) => {
    setRemoteControlClient(clientId);
    addLog(`Started remote control for ${clientId}`);
  };

  const stopRemoteControl = () => {
    setRemoteControlClient(null);
    addLog("Stopped remote control");
  };

  const handleRemoteMouseMove = (e: React.MouseEvent<HTMLImageElement>, clientId: string) => {
    if (remoteControlClient !== clientId) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const x = Math.round((e.clientX - rect.left) / rect.width * 1920); // Assume 1920 width
    const y = Math.round((e.clientY - rect.top) / rect.height * 1080); // Assume 1080 height
    socket?.emit("remote-mouse-move", { clientId, x, y });
  };

  const handleRemoteClick = (e: React.MouseEvent<HTMLImageElement>, clientId: string) => {
    if (remoteControlClient !== clientId) return;
    e.preventDefault();
    const button = e.button === 2 ? "right" : e.button === 1 ? "middle" : "left";
    socket?.emit("remote-mouse-click", { clientId, button });
  };

  const getThumbnailClass = () => {
    switch (thumbnailSize) {
      case "small":
        return "thumb-small";
      case "large":
        return "thumb-large";
      default:
        return "thumb-medium";
    }
  };

  // Login
  if (!isLoggedIn) {
    return (
      <div className="login-page">
        <div className="login-box">
          <div className="login-logo">🖥️</div>
          <h1>Quản Lý Phòng Máy</h1>
          <div className="server-input">
            <label>Server IP:</label>
            <input
              type="text"
              placeholder="localhost"
              value={serverIp}
              onChange={(e) => setServerIp(e.target.value)}
            />
          </div>
          <input
            type="text"
            placeholder="Tên đăng nhập"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
          />
          <input
            type="password"
            placeholder="Mật khẩu"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleLogin()}
          />
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
          <div className={`status-dot ${status.includes("Connected") || status.includes("chia sẻ") ? "online" : ""}`}></div>
          <span>{status}</span>
        </div>
        <div className="client-content">
          <div className="client-icon">📺</div>
          <h2>Client Mode</h2>
          <p>Server: {serverIp}:3001</p>
          <p className="client-note">{status}</p>
        </div>
        {/* Debug Log Panel */}
        <div className="debug-panel">
          <h4>Debug Log:</h4>
          <div className="debug-logs">
            {debugLogs.map((log, i) => (
              <div key={i} className="debug-line">{log}</div>
            ))}
            {debugLogs.length === 0 && <div className="debug-line">No logs yet...</div>}
          </div>
        </div>
      </div>
    );
  }

  // Admin View
  const clientsArray = Array.from(clients.values());
  const lockedCount = clientsArray.filter((c) => c.isLocked).length;

  // Remote control fullscreen view
  if (remoteControlClient) {
    const client = clients.get(remoteControlClient);
    return (
      <div className="remote-control-view">
        <div className="remote-header">
          <span>🖱️ Điều khiển: {client?.name} ({client?.ip})</span>
          <button onClick={stopRemoteControl}>✕ Đóng</button>
        </div>
        <div className="remote-screen">
          {client?.screenData ? (
            <img
              src={client.screenData}
              alt="Remote"
              onMouseMove={(e) => handleRemoteMouseMove(e, remoteControlClient)}
              onClick={(e) => handleRemoteClick(e, remoteControlClient)}
              onContextMenu={(e) => { e.preventDefault(); handleRemoteClick(e, remoteControlClient); }}
            />
          ) : (
            <div className="no-video">Đang kết nối...</div>
          )}
        </div>
      </div>
    );
  }

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
          <button
            className="tool-btn"
            onClick={() => selectedClient && lockClient(selectedClient)}
            disabled={!selectedClient}
            title="Khóa máy đã chọn"
          >
            <span className="tool-icon">🖥️🔒</span>
            <span className="tool-label">Khóa máy</span>
          </button>
          <button
            className="tool-btn"
            onClick={() => selectedClient && unlockClient(selectedClient)}
            disabled={!selectedClient}
            title="Mở khóa máy đã chọn"
          >
            <span className="tool-icon">🖥️🔓</span>
            <span className="tool-label">Mở máy</span>
          </button>
        </div>
        <div className="toolbar-group">
          <div className="toolbar-divider"></div>
          <button
            className={`tool-btn ${viewMode === "grid" ? "active" : ""}`}
            onClick={() => setViewMode("grid")}
            title="Xem dạng lưới"
          >
            <span className="tool-icon">▦</span>
          </button>
          <button
            className={`tool-btn ${viewMode === "list" ? "active" : ""}`}
            onClick={() => setViewMode("list")}
            title="Xem dạng danh sách"
          >
            <span className="tool-icon">☰</span>
          </button>
          <div className="toolbar-divider"></div>
          <select
            value={thumbnailSize}
            onChange={(e) => setThumbnailSize(e.target.value as any)}
            className="size-select"
          >
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
              <div
                key={c.id}
                className={`client-item ${c.isSelected ? "selected" : ""} ${c.isLocked ? "locked" : ""}`}
                onClick={() => selectClient(c.id)}
              >
                <span className="client-icon-small">{c.isLocked ? "🔒" : "🖥️"}</span>
                <div className="client-info">
                  <div className="client-name">{c.name}</div>
                  <div className="client-ip">{c.ip}</div>
                </div>
                <span className={`client-status-dot ${c.screenData ? "streaming" : ""}`}></span>
              </div>
            ))}
            {clientsArray.length === 0 && <div className="no-clients">Chưa có máy kết nối</div>}
          </div>
          <div className="sidebar-footer">
            <div className="server-info">
              {myIp && <small>Server IP: {myIp}:3001</small>}
              {!myIp && <small>Server: localhost:3001</small>}
            </div>
          </div>
        </div>

        {/* Screen Grid */}
        <div className="screen-area">
          {viewMode === "grid" ? (
            <div className={`screen-grid ${getThumbnailClass()}`}>
              {clientsArray.map((c) => (
                <div
                  key={c.id}
                  className={`screen-card ${c.isSelected ? "selected" : ""} ${c.isLocked ? "locked" : ""}`}
                  onClick={() => selectClient(c.id)}
                >
                  <div className="screen-header">
                    <span>{c.name}</span>
                    {c.isLocked && <span className="lock-badge">🔒</span>}
                  </div>
                  <div className="screen-view">
                    {c.screenData ? (
                      <img src={c.screenData} alt={c.name} />
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
                    <button 
                      className="remote-btn"
                      onClick={(e) => { e.stopPropagation(); startRemoteControl(c.id); }}
                    >
                      🖱️
                    </button>
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
                      <td>
                        <span className="list-icon">{c.isLocked ? "🔒" : "🖥️"}</span> {c.name}
                      </td>
                      <td>{c.ip}</td>
                      <td>
                        <span className={`status-badge ${c.isLocked ? "locked" : "online"}`}>
                          {c.isLocked ? "Đã khóa" : "Online"}
                        </span>
                      </td>
                      <td>
                        {c.isLocked ? (
                          <button
                            className="action-btn unlock"
                            onClick={(e) => {
                              e.stopPropagation();
                              unlockClient(c.id);
                            }}
                          >
                            Mở khóa
                          </button>
                        ) : (
                          <button
                            className="action-btn lock"
                            onClick={(e) => {
                              e.stopPropagation();
                              lockClient(c.id);
                            }}
                          >
                            Khóa
                          </button>
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

      {/* Debug Panel for Admin */}
      <div className="debug-panel admin-debug">
        <h4>Debug Log:</h4>
        <div className="debug-logs">
          {debugLogs.map((log, i) => (
            <div key={i} className="debug-line">{log}</div>
          ))}
          {debugLogs.length === 0 && <div className="debug-line">No logs yet...</div>}
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

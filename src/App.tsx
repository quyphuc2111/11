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
  const [h264Decoders] = useState<Map<string, any>>(new Map());
  const [h264Canvases] = useState<Map<string, HTMLCanvasElement>>(new Map());

  // LAN Scan & WOL states
  const [lanHosts, setLanHosts] = useState<{ip: string; hasApp: boolean; online: boolean; mac?: string}[]>([]);
  const [isScanning, setIsScanning] = useState(false);
  const [wolMac, setWolMac] = useState("");
  const [showLanPanel, setShowLanPanel] = useState(false);

  const addLog = (msg: string) => {
    const time = new Date().toLocaleTimeString();
    setDebugLogs((prev) => [...prev.slice(-20), `[${time}] ${msg}`]);
  };

  // LAN Scan function
  const scanLan = async () => {
    if (!isTauri) {
      addLog("LAN scan only works in Tauri app");
      return;
    }
    
    setIsScanning(true);
    setLanHosts([]);
    addLog("Starting LAN scan...");
    
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const hosts = await invoke<{ip: string; hasApp: boolean; online: boolean}[]>("scan_lan");
      setLanHosts(hosts);
      addLog(`LAN scan complete: ${hosts.length} hosts found`);
    } catch (e: any) {
      addLog(`LAN scan error: ${e.message || e}`);
    } finally {
      setIsScanning(false);
    }
  };

  // Wake-on-LAN function
  const sendWol = async (mac: string) => {
    if (!isTauri) {
      addLog("WOL only works in Tauri app");
      return;
    }
    
    if (!mac || mac.length < 17) {
      addLog("Invalid MAC address");
      return;
    }
    
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const result = await invoke<string>("wake_on_lan", { macAddress: mac });
      addLog(result);
    } catch (e: any) {
      addLog(`WOL error: ${e.message || e}`);
    }
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

  // Admin: Start server and UDP frame receiver
  useEffect(() => {
    if (!isLoggedIn || role !== "admin") return;
    const startServer = async () => {
      if (!isTauri) return;
      
      // Start Node.js signaling server
      try {
        const { Command } = await import("@tauri-apps/plugin-shell");
        await Command.sidecar("bin/server").spawn();
      } catch (e) {
        console.error(e);
      }
      
      // Get local IP
      try {
        const { Command } = await import("@tauri-apps/plugin-shell");
        const output = await Command.create("get-ip").execute();
        if (output.code === 0) setMyIp(output.stdout.trim());
      } catch (e) {
        console.error(e);
      }
      
      // Start UDP frame receiver
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const { listen } = await import("@tauri-apps/api/event");
        
        await invoke("start_frame_receiver", { port: 3002 });
        addLog("UDP frame receiver started on port 3002");
        
        // Listen for JPEG frames (fallback)
        await listen<[string, string]>("udp-frame", (event) => {
          const [clientIp, dataUrl] = event.payload;
          
          setClients((prev) => {
            const newMap = new Map(prev);
            let found = false;
            newMap.forEach((client, id) => {
              if (client.ip === clientIp || client.ip === 'unknown') {
                newMap.set(id, { ...client, ip: clientIp, screenData: dataUrl });
                found = true;
              }
            });
            
            if (!found && newMap.size === 0) {
              const placeholderId = `udp-${clientIp}`;
              newMap.set(placeholderId, {
                id: placeholderId,
                ip: clientIp,
                name: `PC-${clientIp.split('.').pop()}`,
                screenData: dataUrl,
                isLocked: false,
                isSelected: false
              });
            }
            
            return newMap;
          });
        });

        // Listen for H.264 frames
        await listen<[string, string]>("h264-frame", async (event) => {
          const [clientIp, base64Data] = event.payload;
          
          try {
            // Check if WebCodecs is available
            if (!('VideoDecoder' in window)) {
              addLog("WebCodecs not available, H.264 decode skipped");
              return;
            }

            const h264Data = Uint8Array.from(atob(base64Data), c => c.charCodeAt(0));
            
            // Get or create decoder for this client
            if (!h264Decoders.has(clientIp)) {
              const canvas = document.createElement('canvas');
              canvas.width = 640;
              canvas.height = 360;
              h264Canvases.set(clientIp, canvas);
              
              const ctx = canvas.getContext('2d')!;
              
              const decoder = new (window as any).VideoDecoder({
                output: (frame: any) => {
                  ctx.drawImage(frame, 0, 0, canvas.width, canvas.height);
                  const imageData = canvas.toDataURL('image/jpeg', 0.85);
                  
                  setClients((prev) => {
                    const newMap = new Map(prev);
                    let found = false;
                    newMap.forEach((client, id) => {
                      if (client.ip === clientIp || client.ip === 'unknown') {
                        newMap.set(id, { ...client, ip: clientIp, screenData: imageData });
                        found = true;
                      }
                    });
                    
                    if (!found) {
                      const placeholderId = `h264-${clientIp}`;
                      newMap.set(placeholderId, {
                        id: placeholderId,
                        ip: clientIp,
                        name: `PC-${clientIp.split('.').pop()}`,
                        screenData: imageData,
                        isLocked: false,
                        isSelected: false
                      });
                    }
                    
                    return newMap;
                  });
                  
                  frame.close();
                },
                error: (e: any) => {
                  addLog(`H264 decode error: ${e.message}`);
                }
              });
              
              // Configure decoder for H.264 Baseline profile
              await decoder.configure({
                codec: 'avc1.42001E', // H.264 Baseline Level 3.0
                width: 640,
                height: 360,
                hardwareAcceleration: 'prefer-hardware'
              });
              
              h264Decoders.set(clientIp, { decoder, frameCount: 0 });
              addLog(`H264 decoder created for ${clientIp}`);
            }
            
            const { decoder, frameCount } = h264Decoders.get(clientIp);
            
            // Detect if this is a keyframe (starts with NAL unit type 5 or 7)
            const isKeyframe = h264Data.length > 4 && (
              (h264Data[4] & 0x1F) === 5 || // IDR slice
              (h264Data[4] & 0x1F) === 7    // SPS
            );
            
            const chunk = new (window as any).EncodedVideoChunk({
              type: isKeyframe ? 'key' : 'delta',
              timestamp: frameCount * 33333, // ~30fps timing
              data: h264Data
            });
            
            decoder.decode(chunk);
            h264Decoders.set(clientIp, { decoder, frameCount: frameCount + 1 });
            
          } catch (e: any) {
            // Silent fail for individual frames
          }
        });
        
        addLog("H.264 decoder ready");
      } catch (e: any) {
        addLog(`UDP receiver error: ${e.message}`);
      }
    };
    startServer();
    
    return () => {
      if (isTauri) {
        import("@tauri-apps/api/core").then(({ invoke }) => {
          invoke("stop_frame_receiver");
        });
        // Cleanup H.264 decoders
        h264Decoders.forEach(({ decoder }) => {
          try {
            decoder.close();
          } catch (e) {}
        });
        h264Decoders.clear();
        h264Canvases.clear();
      }
    };
  }, [isLoggedIn, role]);

  // Socket connection
  useEffect(() => {
    if (!isLoggedIn || !role) return;
    // Admin kết nối localhost vì server chạy trên máy admin
    // Client kết nối đến IP của máy admin
    const targetUrl = role === "admin" 
      ? "http://localhost:3001" 
      : `http://${serverIp}:3001`;
    addLog(`Connecting to: ${targetUrl}`);
    const newSocket = io(targetUrl, { 
      autoConnect: false,
      reconnection: true,
      reconnectionAttempts: 10,
      reconnectionDelay: 1000,
      timeout: 10000
    });
    setSocket(newSocket);
    return () => {
      newSocket.disconnect();
    };
  }, [serverIp, isLoggedIn, role]);

  // Client: Start screen capture with UDP streaming
  useEffect(() => {
    if (!isLoggedIn || role !== "client" || !socket) return;

    let cleanup: (() => void) | null = null;
    let captureStarted = false;

    const startCapture = async () => {
      if (captureStarted) return;
      captureStarted = true;

      addLog(`Starting capture... isTauri: ${isTauri}`);

      if (isTauri) {
        try {
          addLog("Importing Tauri APIs...");
          const { invoke } = await import("@tauri-apps/api/core");
          const { listen } = await import("@tauri-apps/api/event");

          // Try UDP streaming first (better performance, no Socket.IO lag)
          try {
            addLog("Starting UDP stream...");
            const serverAddr = `${serverIp}:3002`; // UDP port
            await invoke("start_stream", { 
              serverAddr, 
              fps: 15 
            });
            addLog(`UDP stream started to ${serverAddr}`);
            setStatus("Đang stream UDP (15 FPS)");

            cleanup = () => {
              invoke("stop_stream");
            };
            return;
          } catch (e: any) {
            addLog(`UDP stream failed: ${e.message}, falling back to Socket.IO`);
          }

          // Fallback to Socket.IO (slower but more reliable)
          addLog("Setting up Socket.IO fallback...");
          let frameCount = 0;
          const unlisten = await listen<string>("screen-frame", (event) => {
            frameCount++;
            if (socket?.connected) {
              socket.emit("screen-frame", event.payload);
              if (frameCount % 10 === 0) {
                addLog(`Frame #${frameCount} sent via Socket.IO`);
              }
            }
          });

          await invoke("start_capture_loop", { intervalMs: 500 });
          addLog("Socket.IO capture started!");
          setStatus("Đang chia sẻ màn hình (Socket.IO)");

          cleanup = () => {
            unlisten();
            invoke("stop_capture_loop");
          };
          return;
        } catch (e: any) {
          addLog(`Rust capture failed: ${e.message || e}`);
        }
      }

      // Fallback: Browser getDisplayMedia
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
  }, [isLoggedIn, role, socket, serverIp]);

  // Socket events
  useEffect(() => {
    if (!socket || !role) return;

    socket.on("connect", () => {
      addLog(`Socket connected! ID: ${socket.id}`);
      setStatus("Connected");
      const name = `PC-${Math.random().toString(36).slice(2, 6).toUpperCase()}`;
      addLog(`Registering as ${role} with name: ${name}`);
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

      // Receive screen frames from clients (JPEG fallback)
      socket.on("screen-frame", ({ clientId, data }: { clientId: string; data: string }) => {
        const clientIds = Array.from(clients.keys());
        addLog(`Frame from ${clientId} (${data?.length || 0} bytes). Known clients: ${clientIds.join(', ') || 'none'}`);
        setClients((prev) => {
          const newMap = new Map(prev);
          const client = newMap.get(clientId);
          if (client) {
            newMap.set(clientId, { ...client, screenData: data });
            addLog(`Updated screen for ${client.name}`);
          } else {
            addLog(`Client ${clientId} not in list yet, creating placeholder`);
            newMap.set(clientId, {
              id: clientId,
              ip: 'unknown',
              name: `PC-${clientId.slice(0, 4)}`,
              screenData: data,
              isLocked: false,
              isSelected: false
            });
          }
          return newMap;
        });
      });

      // Receive H.264 frames via Socket.IO (from server relay)
      socket.on("h264-frame", async ({ clientId, data }: { clientId: string; data: string; sequence: number }) => {
        try {
          if (!('VideoDecoder' in window)) return;
          
          const h264Data = Uint8Array.from(atob(data), c => c.charCodeAt(0));
          
          if (!h264Decoders.has(clientId)) {
            const canvas = document.createElement('canvas');
            canvas.width = 640;
            canvas.height = 360;
            h264Canvases.set(clientId, canvas);
            const ctx = canvas.getContext('2d')!;
            
            const decoder = new (window as any).VideoDecoder({
              output: (frame: any) => {
                ctx.drawImage(frame, 0, 0, canvas.width, canvas.height);
                const imageData = canvas.toDataURL('image/jpeg', 0.85);
                setClients((prev) => {
                  const newMap = new Map(prev);
                  const client = newMap.get(clientId);
                  if (client) {
                    newMap.set(clientId, { ...client, screenData: imageData });
                  }
                  return newMap;
                });
                frame.close();
              },
              error: (e: any) => addLog(`H264 decode error: ${e.message}`)
            });
            
            await decoder.configure({
              codec: 'avc1.42001E',
              width: 640,
              height: 360,
              hardwareAcceleration: 'prefer-hardware'
            });
            
            h264Decoders.set(clientId, { decoder, frameCount: 0 });
          }
          
          const { decoder, frameCount } = h264Decoders.get(clientId);
          const isKeyframe = h264Data.length > 4 && (
            (h264Data[4] & 0x1F) === 5 || (h264Data[4] & 0x1F) === 7
          );
          
          const chunk = new (window as any).EncodedVideoChunk({
            type: isKeyframe ? 'key' : 'delta',
            timestamp: frameCount * 33333,
            data: h264Data
          });
          
          decoder.decode(chunk);
          h264Decoders.set(clientId, { decoder, frameCount: frameCount + 1 });
        } catch (e: any) {
          // Silent fail
        }
      });

      // Receive screen size from client for remote control
      socket.on("screen-size", ({ clientId, width, height }: { clientId: string; width: number; height: number }) => {
        addLog(`Screen size from ${clientId}: ${width}x${height}`);
        setRemoteScreenSize({ width, height });
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

      // Remote control handlers (RustDesk style)
      socket.on("remote-mouse-move", async ({ x, y }: { x: number; y: number }) => {
        if (isTauri) {
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            await invoke("remote_mouse_move", { x, y });
          } catch (e: any) {
            // Silent fail for mouse move
          }
        }
      });

      socket.on("remote-mouse-click", async ({ button }: { button: string }) => {
        addLog(`Received remote click: ${button}`);
        if (isTauri) {
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            await invoke("remote_mouse_click", { button });
            addLog(`Click executed: ${button}`);
          } catch (e: any) {
            addLog(`Mouse click error: ${e.message || e}`);
          }
        }
      });

      socket.on("remote-mouse-scroll", async ({ deltaX, deltaY }: { deltaX: number; deltaY: number }) => {
        if (isTauri) {
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            await invoke("remote_mouse_scroll", { deltaX, deltaY });
          } catch (e: any) {
            addLog(`Scroll error: ${e.message || e}`);
          }
        }
      });

      socket.on("remote-key-press", async ({ key, code, ctrl, alt, shift, meta }: { key: string; code?: string; ctrl?: boolean; alt?: boolean; shift?: boolean; meta?: boolean }) => {
        addLog(`Received remote key: ${key} (code: ${code})`);
        if (isTauri) {
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            await invoke("remote_key_press", { key, code: code || "", ctrl: ctrl || false, alt: alt || false, shift: shift || false, meta: meta || false });
          } catch (e: any) {
            addLog(`Key press error: ${e.message || e}`);
          }
        }
      });

      // Handle screen size request
      socket.on("request-screen-size", async () => {
        addLog("Screen size requested");
        if (isTauri) {
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            const size = await invoke<{ width: number; height: number }>("get_screen_size");
            socket.emit("screen-size-response", size);
            addLog(`Sent screen size: ${size.width}x${size.height}`);
          } catch (e: any) {
            socket.emit("screen-size-response", { width: window.screen.width, height: window.screen.height });
            addLog(`Sent fallback screen size: ${window.screen.width}x${window.screen.height}`);
          }
        } else {
          socket.emit("screen-size-response", { width: window.screen.width, height: window.screen.height });
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
  const [lastMouseMove, setLastMouseMove] = useState(0);
  const [remoteScreenSize, setRemoteScreenSize] = useState({ width: 1920, height: 1080 });

  const startRemoteControl = (clientId: string) => {
    setRemoteControlClient(clientId);
    addLog(`Started remote control for ${clientId}`);
    // Request screen size from client
    socket?.emit("request-screen-size", { clientId });
  };

  const stopRemoteControl = () => {
    setRemoteControlClient(null);
    addLog("Stopped remote control");
  };

  const handleRemoteMouseMove = (e: React.MouseEvent<HTMLDivElement>, clientId: string) => {
    // Throttle mouse move to 60fps max
    const now = Date.now();
    if (now - lastMouseMove < 16) return;
    setLastMouseMove(now);

    const img = e.currentTarget.querySelector('img');
    if (!img) return;
    
    const rect = img.getBoundingClientRect();
    const x = (e.clientX - rect.left) / rect.width * remoteScreenSize.width;
    const y = (e.clientY - rect.top) / rect.height * remoteScreenSize.height;
    
    socket?.emit("remote-mouse-move", { clientId, x, y });
  };

  const handleRemoteClick = (e: React.MouseEvent<HTMLDivElement>, clientId: string) => {
    e.preventDefault();
    const img = e.currentTarget.querySelector('img');
    if (!img) return;
    
    const rect = img.getBoundingClientRect();
    const x = (e.clientX - rect.left) / rect.width * remoteScreenSize.width;
    const y = (e.clientY - rect.top) / rect.height * remoteScreenSize.height;
    const button = e.button === 2 ? "right" : e.button === 1 ? "middle" : "left";
    
    addLog(`Remote click: ${button} at (${Math.round(x)}, ${Math.round(y)}) to ${clientId}`);
    
    // Move then click
    socket?.emit("remote-mouse-move", { clientId, x, y });
    setTimeout(() => {
      socket?.emit("remote-mouse-click", { clientId, button });
    }, 20);
  };

  const handleRemoteScroll = (e: React.WheelEvent<HTMLDivElement>, clientId: string) => {
    e.preventDefault();
    const deltaX = Math.sign(e.deltaX) * -1;
    const deltaY = Math.sign(e.deltaY) * -1;
    socket?.emit("remote-mouse-scroll", { clientId, deltaX, deltaY });
  };

  const handleRemoteKeyDown = (e: React.KeyboardEvent, clientId: string) => {
    e.preventDefault();
    e.stopPropagation();
    
    // ESC to exit remote control
    if (e.key === "Escape" && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      stopRemoteControl();
      return;
    }
    
    addLog(`Remote key: ${e.key} (${e.code}) to ${clientId}`);
    socket?.emit("remote-key-press", { 
      clientId, 
      key: e.key, 
      code: e.code, 
      ctrl: e.ctrlKey, 
      alt: e.altKey, 
      shift: e.shiftKey,
      meta: e.metaKey
    });
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
      <div 
        className="remote-control-view"
        tabIndex={0}
        onKeyDown={(e) => handleRemoteKeyDown(e, remoteControlClient)}
        autoFocus
      >
        <div className="remote-header">
          <span>🖱️ Điều khiển: {client?.name} ({client?.ip}) - {remoteScreenSize.width}x{remoteScreenSize.height}</span>
          <button onClick={stopRemoteControl}>✕ Đóng (ESC)</button>
        </div>
        <div 
          className="remote-screen"
          onMouseMove={(e) => handleRemoteMouseMove(e, remoteControlClient)}
          onClick={(e) => handleRemoteClick(e, remoteControlClient)}
          onContextMenu={(e) => { e.preventDefault(); handleRemoteClick(e, remoteControlClient); }}
          onWheel={(e) => handleRemoteScroll(e, remoteControlClient)}
        >
          {client?.screenData ? (
            <img
              src={client.screenData}
              alt="Remote"
              draggable={false}
              style={{ cursor: 'none', pointerEvents: 'none' }}
            />
          ) : (
            <div className="no-video">Đang kết nối...</div>
          )}
        </div>
        <div className="remote-footer">
          <small>Di chuột và click để điều khiển | Scroll để cuộn | Nhấn phím để gõ | ESC để thoát</small>
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
            className={`tool-btn ${showLanPanel ? "active" : ""}`}
            onClick={() => setShowLanPanel(!showLanPanel)}
            title="Quét mạng LAN"
          >
            <span className="tool-icon">🔍</span>
            <span className="tool-label">Quét LAN</span>
          </button>
          <button
            className="tool-btn"
            onClick={scanLan}
            disabled={isScanning}
            title="Bắt đầu quét"
          >
            <span className="tool-icon">{isScanning ? "⏳" : "📡"}</span>
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

      {/* LAN Panel */}
      {showLanPanel && (
        <div className="lan-panel">
          <div className="lan-header">
            <h4>🌐 Thiết bị trong mạng LAN</h4>
            <button onClick={() => setShowLanPanel(false)}>✕</button>
          </div>
          <div className="lan-wol">
            <input
              type="text"
              placeholder="MAC Address (AA:BB:CC:DD:EE:FF)"
              value={wolMac}
              onChange={(e) => setWolMac(e.target.value)}
            />
            <button onClick={() => sendWol(wolMac)} disabled={!wolMac}>
              ⚡ Wake-on-LAN
            </button>
          </div>
          <div className="lan-hosts">
            {isScanning && <div className="scanning">Đang quét mạng...</div>}
            {lanHosts.length === 0 && !isScanning && (
              <div className="no-hosts">Nhấn 📡 để quét mạng LAN</div>
            )}
            {lanHosts.map((host) => (
              <div key={host.ip} className={`lan-host ${host.hasApp ? "has-app" : ""}`}>
                <span className="host-icon">{host.hasApp ? "✅" : "💻"}</span>
                <span className="host-ip">{host.ip}</span>
                <span className="host-status">
                  {host.hasApp ? "Có App" : "Online"}
                </span>
                {host.mac && (
                  <button 
                    className="wol-btn"
                    onClick={() => sendWol(host.mac!)}
                    title="Wake-on-LAN"
                  >
                    ⚡
                  </button>
                )}
              </div>
            ))}
          </div>
        </div>
      )}

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

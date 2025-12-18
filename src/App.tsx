import { useState, useEffect, useRef } from "react";
import "./App.css";
import io, { Socket } from "socket.io-client";

const DEFAULT_SERVER_URL = "http://localhost:3001";

// Check if running in Tauri
const isTauri = !!(window as any).__TAURI_INTERNALS__;

function App() {
  const [serverIp, setServerIp] = useState("localhost");
  const [socket, setSocket] = useState<Socket | null>(null);
  const [remoteStream, setRemoteStream] = useState<MediaStream | null>(null);
  const [isSharing, setIsSharing] = useState(false);
  const [roomId, setRoomId] = useState("test-room");
  const [joinedRoom, setJoinedRoom] = useState(false);
  const [status, setStatus] = useState("");
  const [messages, setMessages] = useState<string[]>([]);
  const [messageInput, setMessageInput] = useState("");

  const [receivedFileMeta, setReceivedFileMeta] = useState<any>(null);
  const receivedFileChunksRef = useRef<ArrayBuffer[]>([]);
  const [downloadLink, setDownloadLink] = useState<{name: string, url: string} | null>(null);
  const [myIp, setMyIp] = useState<string>("");
  const [connectedUsers, setConnectedUsers] = useState<{id: string, ip: string}[]>([]);

  // Start sidecar server when app launches (Tauri only)
  useEffect(() => {
    const startServer = async () => {
      if (!isTauri) {
        setStatus("Running in browser mode");
        return;
      }
      
      try {
        const { Command } = await import('@tauri-apps/plugin-shell');
        const command = Command.sidecar('bin/server');
        const child = await command.spawn();
        console.log('Sidecar server started with PID:', child.pid);
        setStatus("Local signaling server started.");
      } catch (error) {
        console.error('Failed to start sidecar server:', error);
        // It might be already running or we are in dev mode without sidecar setup
      }

      // Get Local IP (macOS specific for now)
      try {
        const { Command } = await import('@tauri-apps/plugin-shell');
        const output = await Command.create('get-ip').execute();
        if (output.code === 0) {
          setMyIp(output.stdout.trim());
        }
      } catch (e) {
        console.error("Failed to get local IP", e);
      }
    };
    
    startServer();
  }, []);

  const peerConnection = useRef<RTCPeerConnection | null>(null);
  const localStream = useRef<MediaStream | null>(null);
  const dataChannel = useRef<RTCDataChannel | null>(null);

  const isSharingRef = useRef(false);
  const peersRef = useRef<string[]>([]);

  useEffect(() => {
    // Re-initialize socket when serverIp changes, but only if not connected yet or we want to support switching
    // ideally, we create the socket but don't connect until join room, 
    // BUT the current logic connects inside joinRoom, so we just need to update the instance.
    
    // Clean up old socket if exists
    if (socket) {
      socket.disconnect();
    }
    
    // Use user provided IP or default
    const targetUrl = serverIp ? `http://${serverIp}:3001` : DEFAULT_SERVER_URL;

    const newSocket = io(targetUrl, {
      autoConnect: false,
    });
    setSocket(newSocket);

    return () => {
      newSocket.disconnect();
    };
  }, [serverIp]);

  useEffect(() => {
    if (!socket) return;

    const handleUserConnected = (userId: string) => {
      setStatus(`User connected: ${userId}`);
      if (!peersRef.current.includes(userId)) {
        peersRef.current.push(userId);
      }
      if (isSharingRef.current) {
        startCall(userId);
      }
    };

    const handleUserDisconnected = (userId: string) => {
       setStatus(`User disconnected: ${userId}`);
       peersRef.current = peersRef.current.filter(id => id !== userId);
    };

    const handleAllUsers = (users: string[]) => {
      peersRef.current = users;
      console.log("Existing users:", users);
    };

    const handleUpdateUserList = (users: {id: string, ip: string}[]) => {
      setConnectedUsers(users);
      // Sync peersRef to ensure it matches the UI list
      if (socket) {
        const otherUserIds = users
          .filter(u => u.id !== socket.id)
          .map(u => u.id);
        peersRef.current = otherUserIds;
      }
    };

    const handleOffer = async (payload: any) => {
      await handleReceiveOffer(payload);
    };

    const handleAnswer = async (payload: any) => {
      await handleReceiveAnswer(payload);
    };

    const handleIceCandidate = async (payload: any) => {
      await handleNewICECandidateMsg(payload);
    };

    socket.on("connect", () => {
      setStatus("Connected to signaling server");
    });

    socket.on("user-connected", handleUserConnected);
    socket.on("user-disconnected", handleUserDisconnected);
    socket.on("all-users", handleAllUsers);
    socket.on("update-user-list", handleUpdateUserList);
    socket.on("offer", handleOffer);
    socket.on("answer", handleAnswer);
    socket.on("ice-candidate", handleIceCandidate);

    return () => {
      socket.off("connect");
      socket.off("user-connected", handleUserConnected);
      socket.off("user-disconnected", handleUserDisconnected);
      socket.off("all-users", handleAllUsers);
      socket.off("update-user-list", handleUpdateUserList);
      socket.off("offer", handleOffer);
      socket.off("answer", handleAnswer);
      socket.off("ice-candidate", handleIceCandidate);
    };
  }, [socket]); // Re-bind when socket changes

  const joinRoom = async () => {
    if (socket && roomId) {
      socket.connect();
      socket.emit("join-room", roomId);
      setJoinedRoom(true);
      setStatus(`Joined room: ${roomId}`);
      
      // Auto start screen share upon joining
      // await startScreenShare();
    }
  };

  const handleDataChannelMessage = (event: MessageEvent) => {
    const data = event.data;

    if (typeof data === "string") {
      try {
        const parsed = JSON.parse(data);
        if (parsed.type === "chat") {
          setMessages((prev) => [...prev, `Peer: ${parsed.content}`]);
        } else if (parsed.type === "file-meta") {
          setReceivedFileMeta(parsed);
          receivedFileChunksRef.current = [];
          setMessages((prev) => [...prev, `System: Receiving file ${parsed.name} (${parsed.size} bytes)...`]);
        }
      } catch (e) {
        // Fallback for plain text messages if any
        setMessages((prev) => [...prev, `Peer: ${data}`]);
      }
    } else {
      // Binary data (ArrayBuffer)
      if (receivedFileMeta) {
        receivedFileChunksRef.current.push(data);
        const newChunks = receivedFileChunksRef.current;
        
        // Check if download is complete
        const totalReceived = newChunks.reduce((acc, chunk) => acc + chunk.byteLength, 0);
        
        if (totalReceived >= receivedFileMeta.size) {
          // Reassemble file
          const blob = new Blob(newChunks);
          const url = URL.createObjectURL(blob);
          setDownloadLink({ name: receivedFileMeta.name, url });
          setMessages((prev) => [...prev, `System: File ${receivedFileMeta.name} received.`]);
          // Reset state
          // setReceivedFileMeta(null); // Keep it to show download link? Or maybe reset after download?
        }
      }
    }
  };

  const createPeerConnection = (targetUserId: string) => {
    const pc = new RTCPeerConnection({
      iceServers: [
        {
          urls: "stun:stun.l.google.com:19302",
        },
      ],
    });

    pc.onicecandidate = (event) => {
      if (event.candidate && socket) {
        socket.emit("ice-candidate", {
          target: targetUserId,
          candidate: event.candidate,
        });
      }
    };

    pc.ontrack = (event) => {
      setRemoteStream(event.streams[0]);
    };

    // Handle receiving data channel (for the receiver)
    pc.ondatachannel = (event) => {
      const receiveChannel = event.channel;
      receiveChannel.onmessage = handleDataChannelMessage;
      dataChannel.current = receiveChannel;
    };

    peerConnection.current = pc;
    return pc;
  };

  const startScreenShare = async () => {
    try {
      // Check if already sharing to avoid multiple streams
      if (isSharingRef.current) return;

      console.log("Requesting display media...");
      console.log("navigator.mediaDevices:", navigator.mediaDevices);
      console.log("isTauri:", isTauri);
      console.log("isSecureContext:", window.isSecureContext);
      setStatus("Requesting screen access...");
      
      // Check if getDisplayMedia is supported
      if (!navigator.mediaDevices || !navigator.mediaDevices.getDisplayMedia) {
        const hint = isTauri 
          ? "Screen sharing không được hỗ trợ trong Tauri WebView. Vui lòng mở http://localhost:1420 trong Chrome."
          : "Trình duyệt không hỗ trợ screen sharing. Hãy đảm bảo bạn đang dùng HTTPS hoặc localhost (isSecureContext: " + window.isSecureContext + ")";
        throw new Error(hint);
      }
      
      const stream = await navigator.mediaDevices.getDisplayMedia({
        video: true,
        audio: true,
      });
      console.log("Display media acquired:", stream.id);
      
      localStream.current = stream;
      isSharingRef.current = true;
      setIsSharing(true);
      
      const peersToCall = peersRef.current;
      setStatus(peersToCall.length > 0 
        ? `Screen sharing started. Calling ${peersToCall.length} peers...` 
        : `Screen sharing started. Waiting for peers to join...`);
      
      // Initiate calls to all existing peers
      peersToCall.forEach(userId => {
        startCall(userId);
      });
      
    } catch (err: any) {
      console.error("Error starting screen share:", err);
      setStatus(`Screen sharing failed: ${err.name} - ${err.message}`);
      alert(`Error sharing screen: ${err.name} - ${err.message}`);
    }
  };

  const startCall = async (targetUserId: string) => {
    if (!localStream.current) return;

    const pc = createPeerConnection(targetUserId);

    // Add tracks to connection
    localStream.current.getTracks().forEach((track) => {
      pc.addTrack(track, localStream.current!);
    });

    // Create Data Channel (for the initiator)
    const dc = pc.createDataChannel("chat");
    dc.onmessage = handleDataChannelMessage;
    dataChannel.current = dc;

    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);

    socket?.emit("offer", {
      target: targetUserId,
      callerId: socket.id,
      sdp: pc.localDescription,
    });
  };

  const handleReceiveOffer = async (payload: any) => {
    const pc = createPeerConnection(payload.callerId);
    
    await pc.setRemoteDescription(new RTCSessionDescription(payload.sdp));
    const answer = await pc.createAnswer();
    await pc.setLocalDescription(answer);

    socket?.emit("answer", {
      target: payload.callerId, 
      sdp: pc.localDescription,
    });
  };

  const handleReceiveAnswer = async (payload: any) => {
    const pc = peerConnection.current;
    if (pc) {
      await pc.setRemoteDescription(new RTCSessionDescription(payload.sdp));
    }
  };

  const handleNewICECandidateMsg = async (payload: any) => {
    const pc = peerConnection.current;
    if (pc) {
      await pc.addIceCandidate(new RTCIceCandidate(payload.candidate));
    }
  };

  const sendMessage = () => {
    if (dataChannel.current && dataChannel.current.readyState === "open") {
      const payload = { type: "chat", content: messageInput };
      dataChannel.current.send(JSON.stringify(payload));
      setMessages((prev) => [...prev, `Me: ${messageInput}`]);
      setMessageInput("");
    }
  };

  const sendFile = async (file: File) => {
    if (!dataChannel.current || dataChannel.current.readyState !== "open") {
      alert("Connection not ready.");
      return;
    }

    // Send Metadata
    const meta = {
      type: "file-meta",
      name: file.name,
      size: file.size,
    };
    dataChannel.current.send(JSON.stringify(meta));
    setMessages((prev) => [...prev, `Me: Sending file ${file.name}...`]);

    // Send Chunks
    const CHUNK_SIZE = 16 * 1024; // 16KB
    const buffer = await file.arrayBuffer();
    
    for (let i = 0; i < buffer.byteLength; i += CHUNK_SIZE) {
      const chunk = buffer.slice(i, i + CHUNK_SIZE);
      dataChannel.current.send(chunk);
    }
    
    setMessages((prev) => [...prev, `Me: File sent.`]);
  };

  return (
    <main className="container">
      <h1>WebRTC Screen Share & Chat</h1>
      <div className="status">{status}</div>
      
      <div className="controls">
        <div className="input-group">
          <div style={{display: 'flex', flexDirection: 'column'}}>
            <input 
              value={serverIp} 
              onChange={(e) => setServerIp(e.target.value)} 
              placeholder="Server IP (e.g., localhost or 192.168.x.x)" 
              disabled={joinedRoom}
              className="ip-input"
            />
            {myIp && <small style={{fontSize: '0.8em', color: '#666', marginTop: '4px'}}>My Local IP: {myIp}</small>}
          </div>
          <input 
            value={roomId} 
            onChange={(e) => setRoomId(e.target.value)} 
            placeholder="Room ID" 
            disabled={joinedRoom}
          />
        </div>
        <button onClick={joinRoom} disabled={joinedRoom}>
          {joinedRoom ? "Joined" : "Join Room"}
        </button>
       {/* Manual share button */}
      {!isSharing && <button onClick={startScreenShare} disabled={!joinedRoom}>Share Screen</button>}

      {joinedRoom && (
        <div style={{ marginTop: '20px', textAlign: 'left', border: '1px solid #ccc', padding: '10px', borderRadius: '5px' }}>
          <h3>Connected Machines:</h3>
          <ul>
            {connectedUsers.map((u) => (
              <li key={u.id}>
                <strong>{u.id === socket?.id ? "Me" : "Peer"}</strong>: {u.ip} (ID: {u.id.substring(0, 5)}...)
              </li>
            ))}
          </ul>
        </div>
      )}
      </div>

      {/* Video area */}
      <div className="video-container">
        {remoteStream && (
          <video
            autoPlay
            playsInline
            ref={(video) => {
              if (video) video.srcObject = remoteStream;
            }}
            style={{ width: "100%", maxWidth: "800px", border: "1px solid #ccc" }}
          />
        )}
      </div>

      <div className="chat-container">
        <h3>Chat</h3>
        <div className="messages">
          {messages.map((msg, idx) => (
            <div key={idx}>{msg}</div>
          ))}
        </div>
        <input
          value={messageInput}
          onChange={(e) => setMessageInput(e.target.value)}
          placeholder="Type a message..."
        />
        <button onClick={sendMessage}>Send</button>
        
        <div className="file-upload">
          <input 
            type="file" 
            onChange={(e) => {
              if (e.target.files && e.target.files[0]) {
                sendFile(e.target.files[0]);
              }
            }} 
          />
        </div>

        {downloadLink && (
          <div className="download-link">
             <a href={downloadLink.url} download={downloadLink.name}>
               Download {downloadLink.name}
             </a>
          </div>
        )}
      </div>
    </main>
  );
}

export default App;

const { Server } = require("socket.io");
const dgram = require("dgram");

const io = new Server(3001, {
  cors: { origin: "*" },
});

// UDP server for H.264 stream
const udpServer = dgram.createSocket("udp4");
const UDP_PORT = 3002;

// Frame reassembly buffer
const frameBuffers = new Map();

udpServer.on("message", (msg, rinfo) => {
  if (msg.length < 8) return;

  // Parse header
  const sequence = msg.readUInt32BE(0);
  const chunkIndex = msg.readUInt16BE(4);
  const totalChunks = msg.readUInt16BE(6);
  const payload = msg.slice(8);

  const clientKey = `${rinfo.address}:${rinfo.port}`;
  
  if (!frameBuffers.has(clientKey)) {
    frameBuffers.set(clientKey, { sequence: -1, chunks: [], total: 0 });
  }

  const buffer = frameBuffers.get(clientKey);

  // New frame started
  if (sequence !== buffer.sequence) {
    buffer.sequence = sequence;
    buffer.chunks = new Array(totalChunks);
    buffer.total = totalChunks;
    buffer.received = 0;
  }

  // Store chunk
  if (chunkIndex < buffer.total && !buffer.chunks[chunkIndex]) {
    buffer.chunks[chunkIndex] = payload;
    buffer.received++;

    // Frame complete - forward to admins
    if (buffer.received === buffer.total) {
      const h264Frame = Buffer.concat(buffer.chunks);
      
      // Find client socket by IP
      let clientSocketId = null;
      clients.forEach((data, id) => {
        if (data.ip === rinfo.address) {
          clientSocketId = id;
        }
      });

      if (clientSocketId) {
        // Forward H.264 frame to admins
        admins.forEach((_, adminId) => {
          io.to(adminId).emit("h264-frame", {
            clientId: clientSocketId,
            data: h264Frame.toString("base64"),
            sequence
          });
        });
      }

      // Reset buffer
      buffer.chunks = [];
      buffer.received = 0;
    }
  }
});

udpServer.on("listening", () => {
  console.log(`UDP H.264 receiver on port ${UDP_PORT}`);
});

udpServer.bind(UDP_PORT);

console.log("Signaling server running on port 3001");

const admins = new Map();
const clients = new Map();

function getClientIp(socket) {
  let ip = socket.handshake.address;
  if (ip.startsWith("::ffff:")) ip = ip.substr(7);
  return ip;
}

function broadcastClientList() {
  const clientList = Array.from(clients.entries()).map(([id, data]) => ({
    id,
    ip: data.ip,
    name: data.name,
  }));

  admins.forEach((_, adminId) => {
    io.to(adminId).emit("client-list", clientList);
  });
}

io.on("connection", (socket) => {
  const ip = getClientIp(socket);
  console.log("Connected:", socket.id, "IP:", ip);

  socket.on("register", ({ role, name }) => {
    if (role === "admin") {
      admins.set(socket.id, { ip });
      console.log("Admin registered:", socket.id);
      broadcastClientList();
    } else if (role === "client") {
      clients.set(socket.id, { ip, name: name || `PC-${socket.id.substr(0, 4)}`, isLocked: false });
      console.log("Client registered:", socket.id, name);
      broadcastClientList();
    }
  });

  socket.on("offer", (payload) => {
    admins.forEach((_, adminId) => {
      io.to(adminId).emit("offer", { ...payload, callerId: socket.id });
    });
  });

  socket.on("answer", (payload) => {
    io.to(payload.target).emit("answer", payload);
  });

  socket.on("ice-candidate", (payload) => {
    if (payload.target) {
      io.to(payload.target).emit("ice-candidate", { ...payload, from: socket.id });
    } else {
      admins.forEach((_, adminId) => {
        io.to(adminId).emit("ice-candidate", { ...payload, from: socket.id });
      });
    }
  });

  // Forward screen frames from client to admin
  socket.on("screen-frame", (data) => {
    const adminCount = admins.size;
    console.log(`Frame from ${socket.id} (${data?.length || 0} bytes) -> ${adminCount} admins`);
    if (adminCount === 0) {
      console.log("WARNING: No admins connected to receive frames!");
    }
    admins.forEach((adminData, adminId) => {
      console.log(`  Forwarding to admin ${adminId}`);
      io.to(adminId).emit("screen-frame", { clientId: socket.id, data });
    });
  });

  socket.on("lock-client", ({ clientId, message }) => {
    const client = clients.get(clientId);
    if (client) {
      client.isLocked = true;
      io.to(clientId).emit("lock", { message });
      console.log("Locked:", clientId);
    }
  });

  socket.on("unlock-client", ({ clientId }) => {
    const client = clients.get(clientId);
    if (client) {
      client.isLocked = false;
      io.to(clientId).emit("unlock");
      console.log("Unlocked:", clientId);
    }
  });

  socket.on("lock-all", ({ message }) => {
    clients.forEach((client, clientId) => {
      client.isLocked = true;
      io.to(clientId).emit("lock", { message });
    });
    console.log("Locked all clients");
  });

  socket.on("unlock-all", () => {
    clients.forEach((client, clientId) => {
      client.isLocked = false;
      io.to(clientId).emit("unlock");
    });
    console.log("Unlocked all clients");
  });

  // Remote control events (RustDesk style)
  socket.on("remote-mouse-move", ({ clientId, x, y }) => {
    io.to(clientId).emit("remote-mouse-move", { x, y });
  });

  socket.on("remote-mouse-click", ({ clientId, button }) => {
    console.log("Remote click:", clientId, button);
    io.to(clientId).emit("remote-mouse-click", { button });
  });

  socket.on("remote-mouse-scroll", ({ clientId, deltaX, deltaY }) => {
    io.to(clientId).emit("remote-mouse-scroll", { deltaX, deltaY });
  });

  socket.on("remote-key-press", ({ clientId, key, code, ctrl, alt, shift, meta }) => {
    console.log("Remote key:", clientId, key, code);
    io.to(clientId).emit("remote-key-press", { key, code, ctrl, alt, shift, meta });
  });

  // Screen size request/response
  socket.on("request-screen-size", ({ clientId }) => {
    console.log("Screen size request for:", clientId);
    io.to(clientId).emit("request-screen-size");
  });

  socket.on("screen-size-response", (data) => {
    console.log("Screen size response:", data);
    // Forward to all admins
    admins.forEach((_, adminId) => {
      io.to(adminId).emit("screen-size", { clientId: socket.id, ...data });
    });
  });

  socket.on("disconnect", () => {
    console.log("Disconnected:", socket.id);
    if (admins.has(socket.id)) admins.delete(socket.id);
    if (clients.has(socket.id)) {
      clients.delete(socket.id);
      broadcastClientList();
    }
  });
});

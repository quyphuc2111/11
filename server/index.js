const { Server } = require("socket.io");

const io = new Server(3001, {
  cors: {
    origin: "*", // Allow all origins for development
  },
});

console.log("Signaling server running on port 3001");

const socketIdToIp = {};

io.on("connection", (socket) => {
  let ip = socket.handshake.address;
  if (ip.startsWith("::ffff:")) {
    ip = ip.substr(7);
  }
  socketIdToIp[socket.id] = ip;
  console.log("User connected:", socket.id, "IP:", ip);

  socket.on("join-room", (roomId) => {
    socket.join(roomId);
    console.log(`User ${socket.id} joined room ${roomId}`);
    
    // Get all other users in the room
    const clients = io.sockets.adapter.rooms.get(roomId);
    const users = [];
    const allUsersDetails = [];

    if (clients) {
      clients.forEach((clientId) => {
        if (clientId !== socket.id) {
          users.push(clientId);
        }
        allUsersDetails.push({
          id: clientId,
          ip: socketIdToIp[clientId] || 'unknown'
        });
      });
    }
    
    // Send list of existing users to the new user (signaling only)
    socket.emit("all-users", users);
    
    // Notify others
    socket.to(roomId).emit("user-connected", socket.id);

    // Send full user list (including self) to everyone for UI log
    io.to(roomId).emit("update-user-list", allUsersDetails);
  });

  socket.on("offer", (payload) => {
    io.to(payload.target).emit("offer", payload);
  });

  socket.on("answer", (payload) => {
    io.to(payload.target).emit("answer", payload);
  });

  socket.on("ice-candidate", (payload) => {
    io.to(payload.target).emit("ice-candidate", payload);
  });

  socket.on("disconnecting", () => {
    const rooms = [...socket.rooms];
    rooms.forEach((roomId) => {
      socket.to(roomId).emit("user-disconnected", socket.id);

      // Update user list for everyone else in the room
      const clients = io.sockets.adapter.rooms.get(roomId);
      const remainingUsers = [];
      if (clients) {
        clients.forEach((clientId) => {
          if (clientId !== socket.id) {
            remainingUsers.push({
              id: clientId,
              ip: socketIdToIp[clientId] || 'unknown'
            });
          }
        });
      }
      socket.to(roomId).emit("update-user-list", remainingUsers);
    });
  });

  socket.on("disconnect", () => {
    console.log("User disconnected:", socket.id);
    delete socketIdToIp[socket.id];
  });
});

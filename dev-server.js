const express = require('express');
const { ZenithDevController } = require('./index.node'); // Assuming built target via napi
const path = require('path');
const chokidar = require('chokidar');
const { WebSocketServer } = require('ws');
const http = require('http');

const app = express();
const port = 3000;

// Initialize Controller (Points to current project root)
// Default to parent directory (assuming zenith-bundler is inside project or sibling)
// Adjust this based on where you run the server.
const projectRoot = process.env.PROJECT_ROOT || path.resolve(__dirname, '..');
console.log(`[DevServer] Starting Zenith for: ${projectRoot}`);

let controller;
try {
    controller = new ZenithDevController(projectRoot);
} catch (e) {
    console.error("Failed to initialize ZenithDevController:", e);
    console.error("Ensure you have built the native module: 'napi build --platform'");
    process.exit(1);
}

// Create HTTP and WS Server
const server = http.createServer(app);
const wss = new WebSocketServer({ server });

wss.on('connection', (ws) => {
    // console.log('[HMR] Client connected');
});

function notifyHMR() {
    // console.log('[HMR] Broadcasting update signal...');
    wss.clients.forEach((client) => {
        if (client.readyState === 1) { // OPEN
            client.send(JSON.stringify({ type: 'update' }));
        }
    });
}

// Watcher Integration (Tactical Fix)
const watchPath = path.join(projectRoot, 'src/**/*.zen');
console.log(`[Watcher] Watching ${watchPath}`);
const watcher = chokidar.watch(watchPath, {
    ignoreInitial: true
});

watcher.on('change', async (filePath) => {
    // console.log(`[Watcher] File changed: ${filePath}`);
    try {
        // 1. Tell Rust to re-compile (updates the AssetStore in RAM)
        console.time('Rebuild');
        await controller.rebuild();
        console.timeEnd('Rebuild');

        // 2. Tell the Browser to fetch the new assets
        notifyHMR();
    } catch (e) {
        console.error("Rebuild failed:", e);
    }
});

// Serve Assets
app.use(async (req, res, next) => {
    // 1. Try to serve from memory store
    const asset = controller.getAsset(req.path);

    if (asset) {
        if (req.path.endsWith('.js')) {
            res.setHeader('Content-Type', 'application/javascript');
        } else if (req.path.endsWith('.css')) {
            res.setHeader('Content-Type', 'text/css');
        }
        return res.send(asset);
    }

    // 2. SPA / HTML Fallback
    if (req.path === '/' || !path.extname(req.path)) {
        res.send(`
<!DOCTYPE html>
<html>
<head>
    <title>Zenith Dev</title>
    <script type="module" src="/index.js"></script>
    <link rel="stylesheet" href="/zenith.css">
    <script>
    // HMR Client
    const ws = new WebSocket('ws://' + location.host);
    ws.onmessage = (event) => {
        const msg = JSON.parse(event.data);
        if (msg.type === 'update') {
            console.log('[HMR] Update received. Reloading...');
            location.reload(); 
        }
    };
    ws.onopen = () => console.log("[Zenith] HMR Connected");
    </script>
</head>
<body>
    <div id="app"></div>
</body>
</html>
        `);
        return;
    }

    next();
});

server.listen(port, () => {
    console.log(`Zenith Dev Server running at http://localhost:${port}`);
});

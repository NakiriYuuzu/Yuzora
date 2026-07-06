#!/usr/bin/env node

let buffer = "";

process.stdin.setEncoding("utf8");

process.stdin.on("data", (chunk) => {
  buffer += chunk;
  const lines = buffer.split("\n");
  buffer = lines.pop() ?? "";

  for (const line of lines) {
    handleLine(line);
  }
});

process.stdin.on("end", () => {
  if (buffer.trim().length > 0) {
    handleLine(buffer);
  }
});

function handleLine(line) {
  const trimmed = line.trim();
  if (trimmed.length === 0) {
    return;
  }

  let message;
  try {
    message = JSON.parse(trimmed);
  } catch (error) {
    process.stderr.write(`[fake-acp-agent-0b] invalid JSON: ${String(error)}\n`);
    return;
  }

  if (!Object.prototype.hasOwnProperty.call(message, "id")) {
    return;
  }

  const result = message.method === "initialize"
    ? {
        protocolVersion: 1,
        agentCapabilities: {},
        authMethods: [],
      }
    : {};

  writeMessage({
    jsonrpc: "2.0",
    id: message.id,
    result,
  });
}

function writeMessage(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

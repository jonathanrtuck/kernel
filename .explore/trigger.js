#!/usr/bin/env node
// PostToolUse hook: detects writes to design files and spawns background agents.
//
// When spec.md or .explore/state/question.md is modified, this script spawns
// run-agents.js as a detached process. The hook exits immediately (within
// timeout); the agents run independently.

const { spawn } = require("node:child_process");
const path = require("node:path");
const fs = require("node:fs");

// Files that trigger agent runs, mapped to which agents they trigger.
// Values match the "triggers" arrays in config.json.
const TRIGGER_FILES = [
  "design/spec.md",
  "design/graph.d2",
  ".explore/state/question.md",
];

const stdinTimeout = setTimeout(() => process.exit(0), 4_000);
let input = "";

process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  input += chunk;
});
process.stdin.on("end", () => {
  clearTimeout(stdinTimeout);

  try {
    const data = JSON.parse(input);
    const toolName = data.tool_name;

    if (toolName !== "Write" && toolName !== "Edit") process.exit(0);

    const filePath = data.tool_input?.file_path || "";
    const cwd = data.cwd || process.cwd();
    // Normalize to relative path for matching.
    const relative = path.relative(cwd, filePath);
    // Check if this file is a trigger.
    const isTrigger = TRIGGER_FILES.some(
      (triggerFile) =>
        relative === triggerFile || relative.endsWith(triggerFile),
    );

    if (!isTrigger) process.exit(0);

    // Check for lock file — don't spawn if agents are already running.
    const lockPath = path.join(cwd, ".explore", ".lock");

    if (fs.existsSync(lockPath)) {
      const lockAge = Date.now() - fs.statSync(lockPath).mtimeMs;

      // Stale lock (older than 5 minutes) — remove and continue.
      if (lockAge > 5 * 60 * 1_000) fs.unlinkSync(lockPath);
      else process.exit(0);
    }

    // Spawn run-agents.js detached.
    const runner = path.join(cwd, ".explore", "run-agents.js");
    const child = spawn("node", [runner, relative], {
      cwd,
      detached: true,
      env: {
        ...process.env,
        BRAIN_CWD: cwd,
      },
      stdio: "ignore",
    });

    child.unref();

    // Output context so the conversation knows agents were triggered.
    process.stdout.write(
      JSON.stringify({
        additionalContext: `[brain] Agents triggered by change to ${relative}. Results will appear in .explore/state/ when complete.`,
      }),
    );
  } catch {
    // Silent failure — don't break the session.
  }

  process.exit(0);
});

#!/usr/bin/env node
// Spawns design agents based on which file triggered the run.
//
// Usage: node run-agents.js <trigger-file>
//
// Reads config.json to determine which agents to run. Each agent is invoked
// via `claude -p` with its definition as an appended system prompt. Agents
// run sequentially to avoid overwhelming the API. Results are written to
// .brain/state/ by the agents themselves.

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const cwd = process.env.BRAIN_CWD || process.cwd();
const brainDir = path.join(cwd, ".brain");
const triggerFile = process.argv[2];

const log = (msg) =>
  fs.appendFileSync(
    path.join(brainDir, "orchestrator.log"),
    `[${new Date().toISOString()}] ${msg}\n`,
  );

const main = () => {
  // Acquire lock.
  const lockPath = path.join(brainDir, ".lock");

  fs.writeFileSync(lockPath, String(process.pid));

  try {
    // Read config.
    const config = JSON.parse(
      fs.readFileSync(path.join(brainDir, "config.json"), "utf8"),
    );
    // Filter agents by trigger file.
    const agents = config.agents.filter((agent) =>
      agent.triggers.some(
        (trigger) => triggerFile === trigger || triggerFile.endsWith(trigger),
      ),
    );

    if (agents.length === 0)
      return log(`No agents matched trigger: ${triggerFile}`);

    log(
      `Triggered by: ${triggerFile} — running ${agents.length} agent(s): ${agents.map(({ name }) => name).join(", ")}`,
    );

    for (const agent of agents) runAgent(agent);

    log("All agents complete.");
  } catch (err) {
    log(`ERROR: ${err.message}`);
  } finally {
    // Release lock.
    try {
      fs.unlinkSync(lockPath);
    } catch {}
  }
};

function runAgent(agent) {
  const defPath = path.join(brainDir, agent.definition);

  if (!fs.existsSync(defPath)) {
    log(`SKIP ${agent.name}: definition not found at ${defPath}`);
    return;
  }

  const systemPrompt = fs.readFileSync(defPath, "utf8");
  const tools = agent.tools.join(",");
  // Build the prompt. Tell the agent what triggered this run.
  const prompt = [
    `Triggered by a change to: ${triggerFile}`,
    `Write your report to ${agent.output}`,
    "Work autonomously. Read the files you need, analyze, and write your report.",
  ].join("\n");

  log(`START ${agent.name} (model: ${agent.model})`);

  const startTime = Date.now();

  try {
    // execFileSync bypasses the shell — no escaping issues with backticks,
    // quotes, or newlines in the system prompt markdown.
    execFileSync(
      "claude",
      [
        "-p",
        "--append-system-prompt",
        systemPrompt,
        "--model",
        agent.model,
        "--tools",
        tools,
        "--dangerously-skip-permissions",
        "--no-session-persistence",
        "--max-budget-usd",
        "1.00",
        prompt,
      ],
      {
        cwd,
        timeout: agent.timeout || 180_000,
        stdio: ["pipe", "pipe", "pipe"],
        env: { ...process.env },
      },
    );

    const elapsed = ((Date.now() - startTime) / 1_000).toFixed(1);

    log(`COMPLETE ${agent.name} (${elapsed}s)`);
  } catch (err) {
    const elapsed = ((Date.now() - startTime) / 1_000).toFixed(1);
    const stderr = err.stderr ? err.stderr.toString().slice(0, 300) : "";

    log(
      err.killed
        ? `TIMEOUT ${agent.name} (${elapsed}s)`
        : `FAIL ${agent.name} (${elapsed}s): ${err.message.slice(0, 200)}${stderr ? " | stderr: " + stderr : ""}`,
    );
  }
}

main();

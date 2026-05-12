const BASE_URL = "http://127.0.0.1:8080/v1/chat/completions";

const TOOL_NAMES = ["readFile", "read_file", "file_read"];

function computePerplexity(tokenLogprobs) {
  // ppl = exp( - (1/N) * sum_i log p_i )
  if (!tokenLogprobs || tokenLogprobs.length === 0) return NaN;
  const sum = tokenLogprobs.reduce((a, b) => a + b, 0);
  const avgNegLogprob = -sum / tokenLogprobs.length;
  return Math.exp(avgNegLogprob);
}

function buildToolDef(name) {
  return {
    type: "function",
    function: {
      name,
      description:
        "Reads the contents of a text file from disk so you can summarize or analyse it.",
      parameters: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "Absolute or relative path to the file to read.",
          },
        },
        required: ["path"],
        additionalProperties: false,
      },
    },
  };
}

async function measureToolNamePerplexity(toolName) {
  const toolDef = buildToolDef(toolName);

  const userContent = "Could you explain what's in the file /home/user/report.txt and highlight the most important sections?";

  const payload = {
    model: "default",
    messages: [
      { role: "system", content: "You are an AI assistant that can use tools to read files when needed." },
      { role: "user", content: userContent },
    ],
    tools: [toolDef],
    max_tokens: 512,
    temperature: 0.6,
    logprobs: true,
    top_logprobs: 1,
  };

  const resp = await fetch(BASE_URL, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      // Set an API key header if your server enforces it; many llama.cpp setups ignore it.
      // "Authorization": "Bearer no-key",
    },
    body: JSON.stringify(payload),
  });

  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(
      `HTTP ${resp.status} for tool ${toolName}: ${text.slice(0, 500)}`
    );
  }

  const data = await resp.json();
  const choice = data.choices?.[0];
  if (!choice) {
    throw new Error(`No choices returned for tool ${toolName}`);
  }

  const start = choice.logprobs.content.findIndex(lp => lp.token === "<tool_call>")
  const toks = choice.logprobs.content.slice(start);
  // console.log(toks.map(lp => lp.token));
  // console.log(toks.length);
  // console.log(choice.message.tool_calls);

  const tokenLogprobs = [];
  for (const entry of toks) {
    const lp = entry?.logprob;
    if (typeof lp === "number") tokenLogprobs.push(lp);
  }

  const ppl = computePerplexity(tokenLogprobs);
  return { ppl, nTokens: tokenLogprobs.length };
}

function stats(values) {
  if (values.length === 0) return { avg: NaN, min: NaN, max: NaN };
  const sum = values.reduce((a, b) => a + b, 0);
  const avg = sum / values.length;
  const min = Math.min(...values.filter((v) => !Number.isNaN(v)));
  const max = Math.max(...values.filter((v) => !Number.isNaN(v)));
  return { avg, min, max };
}

async function main() {
  const ROUNDS = 10;
  const results = [];

  for (const name of TOOL_NAMES) {
    const pplValues = [];
    let nTokens = 0;

    for (let round = 0; round < ROUNDS; round++) {
      try {
        const { ppl, nTokens: nt } = await measureToolNamePerplexity(name);
        pplValues.push(ppl);
        nTokens = nt;
      } catch (err) {
        console.error(`  Round ${round + 1} error for "${name}":`, err.message);
      }
    }

    const { avg, min, max } = stats(pplValues);
    results.push({ name, avg, min, max, nTokens, rounds: pplValues.length });
  }

  // Print Markdown table
  console.log("\n### Tool name perplexity comparison (" + ROUNDS + " rounds, avg ± min/max)\n");
  console.log("| Tool name  | Rounds | Tokens | Avg PPL  | Min PPL  | Max PPL  |");
  console.log("|-----------|--------|--------|------------|------------|------------|");
  for (const { name, avg, min, max, nTokens, rounds } of results) {
    const avgStr = Number.isNaN(avg) ? "NaN" : avg.toFixed(8);
    const minStr = Number.isNaN(min) ? "NaN" : min.toFixed(8);
    const maxStr = Number.isNaN(max) ? "NaN" : max.toFixed(8);
    console.log(
      `| ${name.padEnd(9)} | ${String(rounds).padStart(6)} | ${String(nTokens).padStart(6)} | ${avgStr.padStart(12)} | ${minStr.padStart(12)} | ${maxStr.padStart(12)} |`
    );
  }
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
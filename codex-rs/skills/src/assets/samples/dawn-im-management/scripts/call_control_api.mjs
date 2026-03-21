#!/usr/bin/env node

import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { randomUUID } from 'node:crypto';

const DEFAULT_DISCOVERY_PATH = process.env.DAWN_CONTROL_API_DISCOVERY_PATH
  || path.join(os.homedir(), '.dawn', 'runtime', 'control-api.json');
const DEFAULT_TIMEOUT_MS = Number(process.env.DAWN_CONTROL_API_TIMEOUT_MS || 5000);

function usage() {
  console.error(
    'Usage: node call_control_api.mjs <method> [--params <json>] [--params-file <path>] [--discovery <path>] [--timeout-ms <ms>]',
  );
}

function parseArgs(argv) {
  const args = {
    method: '',
    discoveryPath: DEFAULT_DISCOVERY_PATH,
    timeoutMs: DEFAULT_TIMEOUT_MS,
    params: {},
  };

  const positional = [];
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    switch (token) {
      case '--params': {
        const raw = argv[index + 1];
        if (!raw) {
          throw new Error('--params requires a JSON string');
        }
        args.params = JSON.parse(raw);
        index += 1;
        break;
      }
      case '--params-file': {
        const filePath = argv[index + 1];
        if (!filePath) {
          throw new Error('--params-file requires a path');
        }
        args.params = JSON.parse(fs.readFileSync(filePath, 'utf8'));
        index += 1;
        break;
      }
      case '--discovery': {
        const filePath = argv[index + 1];
        if (!filePath) {
          throw new Error('--discovery requires a path');
        }
        args.discoveryPath = filePath;
        index += 1;
        break;
      }
      case '--timeout-ms': {
        const raw = argv[index + 1];
        if (!raw || Number.isNaN(Number(raw))) {
          throw new Error('--timeout-ms requires a number');
        }
        args.timeoutMs = Number(raw);
        index += 1;
        break;
      }
      case '-h':
      case '--help':
        usage();
        process.exit(0);
        break;
      default:
        positional.push(token);
        break;
    }
  }

  if (positional.length !== 1) {
    throw new Error('Exactly one control API method is required');
  }

  args.method = positional[0];
  return args;
}

function readDiscovery(discoveryPath) {
  const raw = fs.readFileSync(discoveryPath, 'utf8');
  const discovery = JSON.parse(raw);
  const endpoint = discovery?.endpoint || discovery?.socketPath || discovery?.pipeName || discovery?.path;
  if (!endpoint || typeof endpoint !== 'string') {
    throw new Error(`Control API discovery file is missing an endpoint: ${discoveryPath}`);
  }
  return { endpoint };
}

async function callControlApi(endpoint, method, params, timeoutMs) {
  const request = {
    id: randomUUID(),
    method,
    params,
  };

  return await new Promise((resolve, reject) => {
    const client = net.createConnection(endpoint);
    const timer = setTimeout(() => {
      client.destroy();
      reject(new Error(`Control API timed out for '${method}' after ${timeoutMs}ms`));
    }, timeoutMs);
    let buffer = '';

    function cleanup() {
      clearTimeout(timer);
      client.removeAllListeners();
    }

    client.on('connect', () => {
      client.write(`${JSON.stringify(request)}\n`);
    });

    client.on('data', (chunk) => {
      buffer += chunk.toString('utf8');
      const newlineIndex = buffer.indexOf('\n');
      if (newlineIndex === -1) {
        return;
      }

      cleanup();
      client.end();
      const line = buffer.slice(0, newlineIndex).trim();
      if (!line) {
        reject(new Error(`Control API returned an empty response for '${method}'`));
        return;
      }

      try {
        const response = JSON.parse(line);
        if (!response.ok) {
          reject(new Error(typeof response.error === 'string' ? response.error : JSON.stringify(response.error)));
          return;
        }
        resolve(response.result);
      } catch (error) {
        reject(error);
      }
    });

    client.on('error', (error) => {
      cleanup();
      reject(new Error(`Control API connection failed for '${method}': ${error.message}`));
    });

    client.on('end', () => {
      if (!buffer.trim()) {
        cleanup();
        reject(new Error(`Control API closed without a response for '${method}'`));
      }
    });
  });
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const { endpoint } = readDiscovery(args.discoveryPath);
  const result = await callControlApi(endpoint, args.method, args.params, args.timeoutMs);
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

main().catch((error) => {
  usage();
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});

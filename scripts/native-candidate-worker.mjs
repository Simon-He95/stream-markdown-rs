import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { parentPort, workerData } from 'node:worker_threads'

const { binary, port, wakeup } = workerData
const signal = new Int32Array(wakeup)
const rust = spawn(binary, [], { stdio: ['pipe', 'pipe', 'inherit'] })
const pending = []
let failed

function reply(response) {
  const request = pending.shift()
  if (!request || request.noReply)
    return
  port.postMessage(response)
  Atomics.add(signal, 0, 1)
  Atomics.notify(signal, 0)
}

function fail(message) {
  if (failed)
    return
  failed = message
  while (pending.length)
    reply({ error: message })
}

rust.on('error', error => fail(error.message))
rust.stdin.on('error', error => fail(error.message))
rust.on('exit', (code, signal) => {
  fail(`Rust Markdown worker exited (${signal ?? code})`)
})

createInterface({ input: rust.stdout }).on('line', (line) => {
  try {
    reply(JSON.parse(line))
  }
  catch (error) {
    fail(`Invalid Rust Markdown reply: ${error.message}`)
  }
})

parentPort.on('message', (message) => {
  pending.push(message)
  if (failed)
    reply({ error: failed })
  else
    rust.stdin.write(`${JSON.stringify(message)}\n`)
})

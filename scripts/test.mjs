import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
const cargo=existsSync(`${homedir()}/.cargo/bin/cargo`)?`${homedir()}/.cargo/bin/cargo`:'cargo';
const env={...process.env};
if(process.platform==='darwin')env.CARGO_TARGET_DIR??=`${homedir()}/Library/Developer/BuildCaches/Claro/macos/debug/Claro`;
const run=spawnSync(cargo,['test','--manifest-path','src-tauri/Cargo.toml','--','--test-threads=1'],{env,stdio:'inherit'});
process.exit(run.status??1);

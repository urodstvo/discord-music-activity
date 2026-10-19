// Сверка версий в трёх местах: package.json, src-tauri/tauri.conf.json,
// src-tauri/Cargo.toml. Опционально сверка с git-тегом:
//   node scripts/check-versions.mjs            — три файла между собой
//   node scripts/check-versions.mjs --tag v0.5.0 — плюс тег
// Выход 0 — ок, 1 — рассинхрон (CI падает).
import { readFileSync } from "node:fs";
import { execSync } from "node:child_process";

const root = new URL("..", import.meta.url);

function readJson(path) {
  return JSON.parse(readFileSync(new URL(path, root), "utf8"));
}

const pkg = readJson("package.json");
const tauriConf = readJson("src-tauri/tauri.conf.json");
const cargoToml = readFileSync(new URL("src-tauri/Cargo.toml", root), "utf8");
const cargoVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];

const versions = {
  "package.json": pkg.version,
  "src-tauri/tauri.conf.json": tauriConf.version,
  "src-tauri/Cargo.toml": cargoVersion ?? "(не найдена)",
};

for (const [file, v] of Object.entries(versions)) {
  console.log(`${file}: ${v}`);
}

const unique = new Set(Object.values(versions));
if (unique.size !== 1) {
  console.error("ОШИБКА: версии не совпадают. Обнови все три файла одинаково.");
  process.exit(1);
}

const tagIndex = process.argv.indexOf("--tag");
if (tagIndex !== -1) {
  const tag = process.argv[tagIndex + 1];
  if (!tag) {
    console.error("ОШИБКА: после --tag нужен тег, например --tag v0.5.0");
    process.exit(1);
  }
  const expected = `v${pkg.version}`;
  if (tag !== expected) {
    console.error(`ОШИБКА: тег ${tag} не совпадает с версией ${expected}.`);
    process.exit(1);
  }
  console.log(`Тег ${tag} совпадает с версией.`);
} else {
  // Без явного тега пробуем вывести текущий тег для информации (не падаем).
  try {
    const describe = execSync("git describe --tags --exact-match HEAD", {
      encoding: "utf8",
    }).trim();
    console.log(`Текущий тег HEAD: ${describe}`);
  } catch {
    console.log("Тега на HEAD нет — пропуск проверки тега.");
  }
}

console.log("Версии синхронны.");

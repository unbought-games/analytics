// Deliberately narrow local generator: JSON Schema DTOs only, no runtime/client dependency.
import {readFile, writeFile} from 'node:fs/promises';
const contract = JSON.parse(await readFile(new URL('../../../contracts/openapi.json', import.meta.url), 'utf8'));
const schemas = contract.components.schemas;
const selected = new Set();
function type(schema) {
  if (schema === true) return 'unknown';
  if (schema === false) return 'never';
  if (schema.$ref) {
    const name = schema.$ref.split('/').at(-1); selected.add(name); return name;
  }
  if (schema.const !== undefined) return JSON.stringify(schema.const);
  if (schema.enum) return schema.enum.map(x => JSON.stringify(x)).join(' | ');
  if (schema.anyOf || schema.oneOf) return (schema.anyOf ?? schema.oneOf).map(type).join(' | ');
  if (Array.isArray(schema.type)) return schema.type.map(t => type({...schema, type: t})).join(' | ');
  switch (schema.type) {
    case 'string': return 'string';
    case 'integer': case 'number': return 'number';
    case 'null': return 'null';
    case 'boolean': return 'boolean';
    case 'array': return `Array<${type(schema.items)}>`;
    case 'object': {
      if (!schema.properties) return `Record<string, ${type(schema.additionalProperties ?? true)}>`;
      return `{\n${Object.entries(schema.properties).map(([key, value]) => `  ${JSON.stringify(key)}${schema.required?.includes(key) ? '' : '?'}: ${type(value)};`).join('\n')}\n}`;
    }
    default: return 'unknown';
  }
}
for (const name of ['IngestBatch', 'IngestAcknowledgement', 'EventEnvelope', 'CollectionPolicy', 'HandoffRequest', 'HandoffResponse', 'RedeemHandoffRequest', 'IdentityResetRequest', 'EventSchemaDefinition']) selected.add(name);
const declarations = new Map();
for (const name of selected) {
  if (!schemas[name]) throw new Error(`Missing schema ${name}`);
  declarations.set(name, `export type ${name} = ${type(schemas[name])};`);
}
const content = '// Generated from contracts/openapi.json; run npm run generate:wire in this package.\n// Event IDs/timestamps are strings; no int64 receipts are converted to numbers.\n' + [...declarations].sort(([a], [b]) => a.localeCompare(b)).map(([, value]) => value).join('\n\n') + '\n';
const path = new URL('../src/wire.ts', import.meta.url);
if (process.argv.includes('--check')) {
  if (await readFile(path, 'utf8') !== content) throw new Error('Generated browser wire types are stale');
} else await writeFile(path, content);

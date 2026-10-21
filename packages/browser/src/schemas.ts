import type {EventSchemaDefinition, PropertyType} from './wire.js';
const string = (max_length: number): PropertyType => ({kind: 'string', required: false, max_length});
/** Register these exact v1 schemas through authenticated administration before loading the SDK. */
export function browserSchemas(propertyId: string): EventSchemaDefinition[] {
  const control = {control_kind: string(32)};
  return [
    {name: 'page_view', category: 'navigation', properties: {}},
    {name: 'control_activate', category: 'interaction', properties: {...control, destination: string(512)}},
    {name: 'form_submit', category: 'form', properties: {...control, method: string(8)}},
    {name: 'form_validation', category: 'form', properties: {...control, result: string(16)}},
    {name: 'disclosure_change', category: 'interaction', properties: {...control, open: {kind: 'boolean', required: false}}},
    {name: 'dialog_change', category: 'interaction', properties: {...control, open: {kind: 'boolean', required: false}}},
    {name: 'dialog_close', category: 'interaction', properties: {...control}},
    {name: 'visibility_change', category: 'visibility', properties: {visibility: string(16)}},
    {name: 'page_restore', category: 'visibility', properties: {}}
  ].map(schema => ({...schema, property_id: propertyId, version: 1, authoritative: false}) as EventSchemaDefinition);
}

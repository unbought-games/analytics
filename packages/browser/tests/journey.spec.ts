import {test,expect} from '@playwright/test';
test('handoff capabilities are removed before initialization and unrelated fragments survive',async({page})=>{
 await page.goto('/fixture#analytics_handoff=synthetic-handoff-123&section=help');
 expect(await page.evaluate(()=> (window as any).Analytics.takeHandoff())).toBe('synthetic-handoff-123');
 expect(new URL(page.url()).hash).toBe('#section=help');
 await page.goto('/fixture#analytics_handoff=one&analytics_handoff=two');
 expect(await page.evaluate(()=> (window as any).Analytics.takeHandoff())).toBeNull();expect(new URL(page.url()).hash).toBe('');
});
test('handoff failure and timeout preserve navigation; capability is fragment-only',async({page})=>{
 await page.goto('/fixture');
 const result=await page.evaluate(async()=>{
  const sdk=(window as any).Analytics;
  return [await sdk.handoffURL({createHandoff:async()=> 'synthetic-handoff-123'},'https://destination.example/play?tab=one'),
   await sdk.handoffURL({createHandoff:async()=>{throw Error('offline')}},'https://destination.example/play'),
   await sdk.handoffURL({createHandoff:()=>new Promise(()=>{})},'https://destination.example/play',10)];
 });
 expect(result).toEqual(['https://destination.example/play?tab=one#analytics_handoff=synthetic-handoff-123','https://destination.example/play','https://destination.example/play']);
});

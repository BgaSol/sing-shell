export function element<T extends HTMLElement=HTMLElement>(id:string):T {
  const result=document.getElementById(id);
  if(!result)throw new Error(`Missing element ${id}`);
  return result as T;
}

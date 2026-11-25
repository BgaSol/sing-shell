import type { Component, UpdateItem, UpdateSnapshot } from './core';
export const components:Component[]=['app','engine'];
export const working=(item?:UpdateItem)=>!!item&&['checking','downloading','preparing','applying'].includes(item.phase);
export const downloadable=(item?:UpdateItem)=>!!item?.candidate&&['available','error'].includes(item.phase)&&item.errorStage!=='check';
export const ready=(updates:UpdateSnapshot)=>components.filter(c=>updates.items[c]?.phase==='ready');
export function label(item?:UpdateItem):string{if(!item)return '尚未检查';return ({idle:'尚未检查',checking:'正在检查',current:'已是最新',available:'有新版本',downloading:'正在下载',preparing:'正在校验与准备',ready:'重启后应用',applying:'正在应用',error:'更新未完成',unsupported:'使用系统安装方式更新',external:'外部核心，手动管理'} as Record<string,string>)[item.phase]??item.phase;}

import type {TFunction} from 'i18next'
export type SettingsSectionId='appearance'|'editor'|'terminal'|'herdr'|'git'|'safety'|'logs'|'about'
export const SETTINGS_GROUPS=[
  {id:'personal',sections:['appearance','editor']},
  {id:'workspace',sections:['terminal','herdr','git']},
  {id:'application',sections:['safety','logs','about']},
] as const
const fields:Partial<Record<SettingsSectionId,{ns:string;keys:string[]}>>={
  appearance:{ns:'workbench',keys:['settings.theme','settings.accentColor','settings.leftSidebarBackground','settings.rightSidebarBackground','settings.botAnimations','settings.language','settings.moveOpenedWorkspaceToTop']},
  editor:{ns:'workbench',keys:['settings.editorFontSize','settings.showMinimap']},
  terminal:{ns:'terminal',keys:['fontFamilyLabel','fontSizeLabel']},
  herdr:{ns:'workbench',keys:['herdrSettings.binarySource']},
  git:{ns:'workbench',keys:['gitSettings.detectionLabel','gitSettings.remoteCheckLabel']},
  safety:{ns:'workbench',keys:['settings.reconcileExternalChanges','settings.confirmDestructiveGitActions','settings.trustedWorkspaces']},
  logs:{ns:'workbench',keys:['settings.logs.filters','settings.logs.actions','settings.logs.results']},
  about:{ns:'workbench',keys:['settings.currentVersion','settings.updates']},
}
const aliases:Record<string,string>={
  'settings.botAnimations':'角色 機器人 夥伴 動畫 效能 bot animation motion performance companion',
  'settings.editorFontSize':'字級 字體 字型 font size typography',
  fontSizeLabel:'字級 字體 字型 font size typography',
  fontFamilyLabel:'字體 字型 font family JetBrains Mono Menlo Cascadia Consolas monospace',
  'settings.theme':'淺色 深色 系統 light dark system',
  'settings.accentColor':'配色 palette accent color',
  'settings.leftSidebarBackground':'左側欄 背景 邊框 霧面 sidebar background border glass',
  'settings.rightSidebarBackground':'右側欄 背景 邊框 霧面 sidebar background border glass',
  'settings.language':'語系 language locale',
}
export function settingsSearchResults(query:string,t:TFunction) {
  const words=query.trim().toLocaleLowerCase().split(/\s+/)
  return SETTINGS_GROUPS.flatMap(group=>group.sections).flatMap(section=>{
    const category=t(`settings.sections.${section}.label`,{ns:'workbench'})
    const sub=t(`settings.sections.${section}.sub`,{ns:'workbench'})
    const entries=[{key:section,label:category,target:null as string|null},...(fields[section]?.keys??[]).map(key=>({key,label:t(key,{ns:fields[section]!.ns}),target:t(key,{ns:fields[section]!.ns})}))]
    return entries.filter(item=>words.every(word=>`${section} ${category} ${sub} ${item.label} ${item.key} ${aliases[item.key]??''}`.toLocaleLowerCase().includes(word))).map(item=>({...item,section,category,sub}))
  })
}

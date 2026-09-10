import {Check,Sun,Moon,Monitor} from 'lucide-react'
import {useTranslation} from 'react-i18next'
import {ToggleGroup,ToggleGroupItem} from '@/components/ui/toggle-group'
import type {ThemePreference} from '@/app/workbench/settingsStorage'

/** A decorative sketch of Yuzora chrome, not a screenshot or live runtime. */
export function SettingsThemePicker({value,onChange}:{value:ThemePreference;onChange:(value:ThemePreference)=>void}) {
  const {t}=useTranslation('settingsDemo')
  return <ToggleGroup type="single" value={value} onValueChange={next=>{if(next)onChange(next as ThemePreference)}} aria-label={t('theme')} className="settings-theme-choices">
    {([{id:'light',icon:Sun},{id:'dark',icon:Moon},{id:'auto',icon:Monitor}] as const).map(({id,icon:Icon})=><ToggleGroupItem key={id} value={id} aria-label={t(`themes.${id}`)} className="settings-theme-choice">
      <span className="settings-theme-sketch" data-theme-preview={id} aria-hidden="true"><span className="theme-sketch-top"><i/><i/><i/></span><span className="theme-sketch-body"><span className="theme-sketch-nav"><i/><i/><i/></span><span className="theme-sketch-content"><i/><i/><i/><span/><span/></span></span></span>
      <span className="settings-theme-caption"><Icon aria-hidden="true"/>{t(`themes.${id}`)}<Check className="theme-choice-check" aria-hidden="true"/></span>
    </ToggleGroupItem>)}
  </ToggleGroup>
}

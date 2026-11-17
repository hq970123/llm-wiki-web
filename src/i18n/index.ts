import i18n from "i18next"
import { initReactI18next } from "react-i18next"
import en from "./en.json"
import it from "./it.json"
import zh from "./zh.json"
import ru from "./ru.json"

// Before the saved preference loads, follow the browser/OS language so
// Chinese users never see an English first screen.
function detectInitialLanguage(): string {
  const nav = typeof navigator !== "undefined" ? navigator.language || "" : ""
  const code = nav.toLowerCase().split("-")[0]
  return ["zh", "it", "ru"].includes(code) ? code : "en"
}

i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    it: { translation: it },
    zh: { translation: zh },
    ru: { translation: ru },
  },
  lng: detectInitialLanguage(),
  fallbackLng: "en",
  interpolation: { escapeValue: false },
})

export default i18n

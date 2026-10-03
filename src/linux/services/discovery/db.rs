use wayclip_core::models::{
    clips::games::{Game, GameRegistry},
    error::WayclipError,
};

#[derive(Clone, Default)]
pub struct ExecutableDb;

impl ExecutableDb {
    pub fn load() -> Result<Self, WayclipError> {
        let _ = GameRegistry::global();
        Ok(Self)
    }

    pub fn by_exe(&self, exe: &str) -> Option<Game> {
        GameRegistry::global().by_exe(exe)
    }
}

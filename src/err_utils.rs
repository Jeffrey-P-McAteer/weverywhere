use std::fmt;
use std::error::Error;

// Error wrapper that includes file and line information
#[derive(Debug)]
pub struct LocationError {
    pub file: &'static str,
    pub line: u32,
    //pub function: &'static str,
    pub function: String,
    pub source: Box<dyn Error + Send + Sync>,
}

impl fmt::Display for LocationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at {}:{}\t{}\n{}", self.file, self.line, self.function, self.source)
    }
}

impl Error for LocationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

// Macro to wrap an error with file and line information
#[macro_export]
macro_rules! loc_err {
    ($err:expr) => {{
        // Hack to get the function name, see https://stackoverflow.com/a/40234666
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        let fn_name = name.strip_suffix("::f").unwrap();
        let mut fn_name = fn_name.replace(module_path!(), "");
        if fn_name.len() > 2 { // remove any '::' at beginning of type name
            fn_name.remove(0);
            fn_name.remove(0);
        }

        let err_box: Box<dyn std::error::Error + Send + Sync> = Box::from($err);

        crate::err_utils::LocationError {
            file: file!(),
            line: line!(),
            function: fn_name,
            source: err_box,
        }
    }};
}

// Alternative macro for use with Result types via map_err
#[macro_export]
macro_rules! map_loc_err {
    () => {
        |e| loc_err!(e)
    };
}


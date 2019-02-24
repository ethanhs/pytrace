#[macro_use]
extern crate cpp;

use pyo3::ffi::{
    PyFrameObject, PyObject, PyObject_GetAttrString, _PyEval_EvalFrameDefault, CO_VARARGS,
    CO_VARKEYWORDS,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyObjectRef, PyString};

use serde::Serialize;

use lazy_static::lazy_static;

use std::borrow::Cow;
use std::boxed::Box;
use std::ffi::CString;
use std::ops::Deref;
use std::os::raw::c_int;
use std::sync::{Arc, Mutex};

type _PyFrameEvalFunction = unsafe extern "C" fn(*mut PyFrameObject, c_int) -> *mut PyObject;

cpp! {{
    #include <stdio.h>
    #include <Python.h>
}}

lazy_static! {
    static ref PATHS: Mutex<Vec<String>> = Mutex::new(Vec::new());
}

#[derive(Serialize, Debug)]
struct Arg {
    name: String,
    typ: String,
    kind: ArgKind,
}

#[derive(Serialize, Debug)]
enum ArgKind {
    Positional,
    StarArgs,
    KeywordOnly,
    StarKwargs,
}

/// Extract the arguments from the frame->f_locals (a mapping of name to value)
/// This is inspired by the code in inspect.py
fn locals_to_args<'a>(
    locals: &'a PyDict,
    argc: usize,
    kwargc: usize,
    coflags: i32,
) -> Arc<Vec<Arg>> {
    let mut args = vec![];
    let mut items = Vec::with_capacity((argc + kwargc) as usize);
    items.extend(locals.iter());
    let positional = &items[..argc];
    let keywordonly = &items[argc..argc + kwargc];
    let varargs = (coflags & CO_VARARGS) != 0;
    let varkwargs = (coflags & CO_VARKEYWORDS) != 0;
    for (pyname, pyval) in positional {
        let name = pyname.to_string();
        let val = pyval.get_type().name();
        args.push(Arg {
            name: name,
            typ: String::from(val.deref()),
            kind: ArgKind::Positional,
        });
    }
    if varargs {
        let (pyname, pyval) = items[argc + kwargc];
        let name = pyname.to_string();
        let val = pyval.get_type().name();
        args.push(Arg {
            name: name,
            typ: String::from(val.deref()),
            kind: ArgKind::StarArgs,
        });
    }
    for (pyname, pyval) in keywordonly {
        let name = pyname.to_string();
        let val = pyval.get_type().name();
        args.push(Arg {
            name: name,
            typ: String::from(val.deref()),
            kind: ArgKind::KeywordOnly,
        });
    }
    if varkwargs {
        let index = if varargs {
            argc + kwargc + 1
        } else {
            argc + kwargc
        };
        let (pyname, pyval) = items[index];
        let name = pyname.to_string();
        let val = pyval.get_type().name();
        args.push(Arg {
            name: name,
            typ: String::from(val.deref()),
            kind: ArgKind::StarKwargs,
        });
    }
    Arc::new(args)
}

#[derive(Serialize, Debug)]
struct FrameInfo<'a> {
    name: &'a str,
    filename: &'a str,
    args: Arc<Vec<Arg>>,
    returns: &'a str,
}

impl<'a> FrameInfo<'a> {
    fn new(
        name: &'a str,
        filename: &'a str,
        returns: &'a str,
        locals: &'a PyDict,
        argc: i32,
        kwargc: i32,
        coflags: i32,
    ) -> FrameInfo<'a> {
        let args = locals_to_args(locals, argc as usize, kwargc as usize, coflags);
        FrameInfo {
            name: name,
            filename: filename,
            args: args,
            returns: returns,
        }
    }
}

/// Get the type of a Python object pointer
fn get_type<'a>(py: Python<'a>, obj: *mut PyObject) -> Cow<'a, str> {
    match unsafe { py.from_borrowed_ptr_or_opt::<PyObjectRef>(obj) } {
        Some(typ) => typ.get_type().name(),
        None => Cow::from("<unknown>"),
    }
}

/// Hook into the Python interpreter. This will do nothing if
/// there is an exception, but otherwise will try to print out information on
/// frames being executed
unsafe extern "C" fn frame_printer(frame: *mut PyFrameObject, exc: c_int) -> *mut PyObject {
    if exc != 0 {
        return _PyEval_EvalFrameDefault(frame, exc);
    }
    let py = Python::assume_gil_acquired();
    let code_obj = *(*frame).f_code;
    let co_name = py
        .from_borrowed_ptr_or_err::<PyObjectRef>(code_obj.co_name)
        .unwrap();
    let co_filename = py
        .from_borrowed_ptr_or_err::<PyObjectRef>(code_obj.co_filename)
        .unwrap();
    let pyname: &PyString = co_name
        .extract()
        .expect("Failed getting string from co_name");
    let pyfile: &PyString = co_filename
        .extract()
        .expect("Failed getting string from co_filename");
    let cname = pyname.to_string().expect("Failed to decode frame name");
    let cfile = pyfile
        .to_string()
        .expect("Failed to decode frame file name");
    let name = cname.deref();
    let file = cfile.deref();

    if name != "<module>" {
        let locals_name = CString::new("f_locals").unwrap();
        let frame_locals = PyObject_GetAttrString(frame as *mut PyObject, locals_name.as_ptr());
        let locals = match py.from_borrowed_ptr_or_opt::<PyObjectRef>(frame_locals) {
            Some(obj) => obj.extract::<&PyDict>().unwrap(),
            None => &PyDict::new(py),
        };
        let ret = _PyEval_EvalFrameDefault(frame, exc);
        let ret_ty = get_type(py, ret);
        let info = FrameInfo::new(
            name,
            file,
            ret_ty.deref(),
            locals,
            code_obj.co_argcount,
            code_obj.co_kwonlyargcount,
            code_obj.co_flags,
        );
        println!("{:?}", info);
        ret
    } else {
        _PyEval_EvalFrameDefault(frame, exc)
    }
}

#[pymodule]
fn pytrace_native(_py: Python, m: &PyModule) -> PyResult<()> {
    /// Hook into the Python interpreter
    #[pyfn(m, "hook")]
    fn hook(_py: Python, path_list: &PyList) -> PyResult<()> {
        let path_strings = path_list.iter().map(|path: &PyObjectRef| {
            let p = path.to_string();
            String::from(p.deref())
        });
        let mut paths = PATHS.lock().unwrap();
        paths.extend(path_strings);
        cpp!(unsafe [] {
            PyThreadState *state = PyThreadState_Get();
            _PyFrameEvalFunction func = state->interp->eval_frame;
            state->interp->eval_frame = rust!(
                fprinter [] -> _PyFrameEvalFunction as "_PyFrameEvalFunction" {
                    frame_printer
                });
        });
        Ok(())
    }

    /// Unhook from the Python interpreter
    #[pyfn(m, "unhook")]
    fn unhook(_py: Python) -> PyResult<()> {
        cpp!(unsafe [] {
            PyThreadState *state = PyThreadState_Get();
            state->interp->eval_frame = _PyEval_EvalFrameDefault;
        });
        Ok(())
    }

    #[pyfn(m, "print_paths")]
    fn print_paths(_py: Python) -> PyResult<()> {
        let paths = PATHS.lock().unwrap();
        println!("{:?}", paths);
        Ok(())
    }

    Ok(())
}
use core::fmt;

pub struct Dot<'a> {
    pub(crate) fmt: &'a mut dyn fmt::Write,
    pub(crate) subgraphs: usize,
    pub(crate) nodes: usize,
    pub(crate) label: Option<&'static str>,
    pub(crate) indent: usize,
}

impl<'a> Dot<'a> {
    pub(crate) fn new<W: fmt::Write>(fmt: &'a mut W) -> Self {
        Self {
            fmt,
            subgraphs: 0,
            nodes: 0,
            label: None,
            indent: 1,
        }
    }

    pub fn subgraph<T, F: FnOnce(&mut Self) -> fmt::Result>(&mut self, f: F) -> fmt::Result {
        let subgraphs = self.subgraphs;
        self.write(format_args!("subgraph cluster_{} {{\n", subgraphs))?;

        self.subgraphs += 1;
        self.indent += 1;

        if let Some(label) = self.label.take() {
            self.write(format_args!("label = {:?};\n", label))?;
        } else {
            self.write(format_args!("label = {:?};\n", core::any::type_name::<T>()))?;
        }

        f(self)?;

        self.indent -= 1;

        self.write("}\n")
    }

    pub(crate) fn node<T, N: fmt::Display>(&mut self, node: N) -> fmt::Result {
        self.write(node)?;

        if let Some(label) = self.label.take() {
            write!(self.fmt, " [label = {:?}]", label)?;
        } else {
            write!(self.fmt, " [label = {:?}]", core::any::type_name::<T>())?;
        }

        writeln!(self.fmt, ";")
    }

    pub fn set_label(&mut self, label: &'static str) {
        self.label = Some(label);
    }

    fn write<F: fmt::Display>(&mut self, f: F) -> fmt::Result {
        for _ in 0..self.indent {
            write!(self.fmt, "  ")?;
        }
        write!(self.fmt, "{}", f)
    }
}
